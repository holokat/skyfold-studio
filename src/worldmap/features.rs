//! Vector layers drawn over the base: inked coastlines with ripple lines, lakes and rivers,
//! contours (atlas), mountain, hill and tree symbols (parchment), roads, building footprints and
//! dashed zone borders.

use std::collections::HashMap;

use bevy::math::Vec2;
use tiny_skia::PathBuilder;

use super::MapStyle;
use super::canvas::{Canvas, Rgba, poly_path, smooth_path, with_alpha};
use super::field::{Grid, Line, chaikin, chain, contours, resample};
use super::render::{Ctx, Progress};
use super::snapshot::TreeKind;
use crate::noise::{Rng, value2};
use crate::roads::RoadKind;

/// What later layers need from this pass.
#[derive(Default)]
pub struct Drawn {
    /// Index contours (atlas): elevation and the line in map px.
    pub index_contours: Vec<(f32, Vec<Vec2>)>,
    /// Each river's centreline in map px (river index, line).
    pub rivers: Vec<(usize, Vec<Vec2>)>,
}

/// Coarse grid of places symbols must keep clear of (water, rivers, roads, POIs).
struct Keepout {
    cell: f32,
    w: usize,
    v: Vec<bool>,
}

impl Keepout {
    fn mark_disc(&mut self, c: Vec2, r: f32) {
        let r2 = r * r;
        let (i0, i1) = (
            ((c.x - r) / self.cell).floor().max(0.0) as usize,
            ((c.x + r) / self.cell).ceil() as usize,
        );
        let (j0, j1) = (
            ((c.y - r) / self.cell).floor().max(0.0) as usize,
            ((c.y + r) / self.cell).ceil() as usize,
        );
        for j in j0..j1.min(self.w) {
            for i in i0..i1.min(self.w) {
                let p = (Vec2::new(i as f32, j as f32) + 0.5) * self.cell;
                if p.distance_squared(c) <= r2 + self.cell * self.cell * 0.5 {
                    self.v[j * self.w + i] = true;
                }
            }
        }
    }
    fn mark_line(&mut self, pts: &[Vec2], r: f32) {
        for p in resample(pts, self.cell * 0.7) {
            self.mark_disc(p, r);
        }
    }
    fn blocked(&self, p: Vec2) -> bool {
        let (i, j) = ((p.x / self.cell) as usize, (p.y / self.cell) as usize);
        i >= self.w || j >= self.w || self.v[j * self.w + i]
    }
}

fn keepout(c: &Ctx) -> Keepout {
    let u = c.w as f32 / 2048.0;
    let cell = 3.0 * u;
    let w = (c.w as f32 / cell).ceil() as usize;
    let mut k = Keepout {
        cell,
        w,
        v: vec![false; w * w],
    };
    for j in 0..w {
        for i in 0..w {
            let p = (Vec2::new(i as f32, j as f32) + 0.5) * cell;
            if c.coast_d(p) > -2.5 * u || c.inland_water(p) {
                k.v[j * w + i] = true;
            }
        }
    }
    for r in &c.inp.rivers {
        let line: Vec<Vec2> = r.pts.iter().map(|p| c.world_to_px(*p)).collect();
        k.mark_line(&line, r.width * 0.5 * c.ppm() + 4.0 * u);
    }
    if c.settings.layers.roads {
        for r in &c.inp.roads {
            let line: Vec<Vec2> = r.line.iter().map(|p| c.world_to_px(*p)).collect();
            k.mark_line(&line, 4.0 * u);
        }
    }
    if c.settings.layers.pois {
        for p in &c.inp.pois {
            k.mark_disc(c.world_to_px(p.pos), 16.0 * u);
        }
    }
    for b in &c.inp.buildings {
        k.mark_disc(c.world_to_px(b.pos), b.radius * c.ppm() + 2.0 * u);
    }
    k
}

/// Coast, ripples, lakes, rivers, then the relief and forest symbols or contours.
pub fn water_and_relief(cv: &mut Canvas, c: &Ctx, progress: &Progress) -> Drawn {
    let u = cv.u;
    let kit = &c.kit;
    let mut drawn = Drawn::default();
    let sat = kit.style == MapStyle::Satellite;
    let to_px_coast = c.k / c.coast_f;

    if !sat {
        // ripple lines echo the shore out at sea (parchment)
        if let Some(rc) = kit.ripple {
            let rings = [
                (5.0, 0.55, 0.85),
                (10.5, 0.40, 0.75),
                (17.0, 0.28, 0.7),
                (25.0, 0.17, 0.65),
            ];
            for (d, a, wdt) in rings {
                for l in contours(&c.coast, d * u) {
                    if l.pts.len() < 6 {
                        continue;
                    }
                    let pts: Vec<Vec2> = l.pts.iter().map(|p| *p * to_px_coast).collect();
                    if let Some(path) = smooth_path(&pts, l.closed) {
                        cv.stroke(&path, wdt * u, with_alpha(rc, a));
                    }
                }
            }
        }
        // the coastline itself
        let coast_lines = contours(&c.coast_h, c.inp.sea);
        for l in &coast_lines {
            let pts: Vec<Vec2> = l.pts.iter().map(|p| *p * to_px_coast).collect();
            if l.length() * to_px_coast < 6.0 * u {
                continue;
            }
            if let Some(path) = smooth_path(&pts, l.closed) {
                if kit.style == MapStyle::Parchment {
                    // a soft shadow line on the land side gives the hand-inked shore some weight
                    cv.stroke(&path, kit.coast_w * u * 2.6, with_alpha(kit.coast, 0.10));
                }
                cv.stroke(&path, kit.coast_w * u, kit.coast);
            }
        }
        // lakes and wide rivers: the water raster's outline
        for l in contours(&c.water, 0.0) {
            if l.length() < 3.0 {
                continue;
            }
            let pts: Vec<Vec2> = l.pts.iter().map(|p| *p * c.k).collect();
            if let Some(path) = smooth_path(&pts, l.closed) {
                cv.stroke(&path, (kit.coast_w * 0.75).max(0.8) * u, kit.water_ink);
            }
        }
    }
    // rivers: widening from the source to the mouth, banks inked, ending at the coast
    for (ri, r) in c.inp.rivers.iter().enumerate() {
        let line: Vec<Vec2> = r.pts.iter().map(|p| c.world_to_px(*p)).collect();
        let mut line = chaikin(&resample(&line, 4.0 * u), false, 2);
        // end where it meets open sea (a bed carved below sea level isn't the sea yet)
        let full = (r.width * c.ppm()).max(1.8 * u);
        if let Some(k) = line
            .iter()
            .position(|p| c.coast_d(*p) > full * 0.5 + 5.0 * u)
        {
            line.truncate(k + 1);
        }
        if line.len() < 2 {
            continue;
        }
        drawn.rivers.push((ri, line.clone()));
        if sat {
            continue;
        }
        if full < 3.5 * u {
            taper(cv, &line, 0.8 * u, full, kit.river, None);
        } else {
            taper(
                cv,
                &line,
                full * 0.45,
                full,
                kit.river_fill,
                Some((kit.river, (1.5 * u).max(1.3))),
            );
        }
    }
    progress.set(0.42, "Relief");

    match kit.style {
        MapStyle::Atlas => drawn.index_contours = contour_lines(cv, c),
        MapStyle::Parchment => {
            sea_marks(cv, c);
            // every symbol in one back-to-front pass, so a tree behind a peak never covers it
            let mut keep = keepout(c);
            let mut symbols = mountains(cv.u, c, &mut keep);
            progress.set(0.5, "Forests");
            symbols.extend(forests(cv.u, c, &keep));
            symbols.sort_by(|a, b| a.base_y().total_cmp(&b.base_y()));
            let mut rng = Rng::new(17);
            for s in symbols {
                match s {
                    Symbol::Peak(p, size, snowy) => {
                        mountain_glyph(cv, p, size, c.kit.ink, &mut rng, snowy)
                    }
                    Symbol::Hill(p, size) => hill_glyph(cv, p, size, c.kit.ink, &mut rng),
                    Symbol::Tree(p, size, kind) => {
                        tree_glyph(cv, p, size, kind, c.kit.ink, &mut rng)
                    }
                }
            }
        }
        _ => {}
    }
    drawn
}

/// Little inked wave marks scattered over the open sea.
fn sea_marks(cv: &mut Canvas, c: &Ctx) {
    let u = cv.u;
    let w = c.w as f32;
    let edge = 40.0 * u;
    let marks = super::field::poisson(
        w,
        w,
        70.0 * u,
        |_| 70.0 * u,
        |p| {
            p.min_element() > edge
                && p.max_element() < w - edge
                && c.coast_d(p) > 45.0 * u
                && value2(p.x / (300.0 * u), p.y / (300.0 * u), 5) > 0.35
        },
        21,
    );
    let ink = with_alpha(c.kit.ripple.unwrap_or(c.kit.ink), 0.45);
    let mut rng = Rng::new(29);
    for p in marks {
        let s = rng.range(0.8, 1.2) * u;
        // a short swell line (crest, trough, crest) with a smaller one tucked under it
        let mut pb = PathBuilder::new();
        let wave = |pb: &mut PathBuilder, o: Vec2, k: f32| {
            pb.move_to(o.x - 10.0 * k, o.y);
            pb.cubic_to(
                o.x - 6.5 * k,
                o.y - 3.2 * k,
                o.x - 3.5 * k,
                o.y - 3.2 * k,
                o.x,
                o.y,
            );
            pb.cubic_to(
                o.x + 3.5 * k,
                o.y + 3.2 * k,
                o.x + 6.5 * k,
                o.y + 3.2 * k,
                o.x + 10.0 * k,
                o.y,
            );
        };
        wave(&mut pb, p, s);
        wave(&mut pb, p + Vec2::new(6.0 * s, 6.0 * s), s * 0.6);
        if let Some(path) = pb.finish() {
            cv.stroke(&path, (0.9 * u).max(0.7), ink);
        }
    }
}

/// A line whose width grows from `w0` at its start to `w1` at its end (a river towards its mouth),
/// optionally with inked edges.
fn taper(cv: &mut Canvas, line: &[Vec2], w0: f32, w1: f32, col: Rgba, banks: Option<(Rgba, f32)>) {
    let n = line.len();
    let total: f32 = line
        .windows(2)
        .map(|w| w[0].distance(w[1]))
        .sum::<f32>()
        .max(1e-3);
    let mut acc = 0.0;
    let mut left = Vec::with_capacity(n);
    let mut right = Vec::with_capacity(n);
    for i in 0..n {
        if i > 0 {
            acc += line[i].distance(line[i - 1]);
        }
        let d = (line[(i + 1).min(n - 1)] - line[i.saturating_sub(1)]).normalize_or(Vec2::X);
        let nrm = Vec2::new(-d.y, d.x);
        let t = (acc / total).powf(0.6);
        let hw = (w0 + (w1 - w0) * t) * 0.5;
        left.push(line[i] + nrm * hw);
        right.push(line[i] - nrm * hw);
    }
    right.reverse();
    if let Some((ink, w)) = banks {
        for side in [&left, &right] {
            if let Some(p) = smooth_path(side, false) {
                cv.stroke(&p, w, ink);
            }
        }
    }
    left.extend(right);
    if let Some(p) = smooth_path(&left, true) {
        cv.fill(&p, col);
    }
}

/// Atlas contours: regular lines, every fifth heavier; returns the index lines for labelling.
fn contour_lines(cv: &mut Canvas, c: &Ctx) -> Vec<(f32, Vec<Vec2>)> {
    let u = cv.u;
    let interval = [5.0f32, 10.0, 20.0, 25.0, 50.0, 100.0, 200.0]
        .into_iter()
        .find(|i| c.hmax / i <= 24.0)
        .unwrap_or(250.0);
    let mut h = c.inp.heights.clone();
    h.blur(1, 1);
    let mut index = Vec::new();
    let mut level = interval;
    let ink = super::canvas::rgb(150, 104, 66);
    while level < c.hmax {
        let major = ((level / interval).round() as i32) % 5 == 0;
        for l in contours(&h, c.inp.sea + level) {
            let pts: Vec<Vec2> = l.pts.iter().map(|p| *p * c.k).collect();
            let len = l.length() * c.k;
            if len < 14.0 * u {
                continue;
            }
            if let Some(path) = smooth_path(&pts, l.closed) {
                if major {
                    cv.stroke(&path, 1.1 * u, with_alpha(ink, 0.85));
                } else {
                    cv.stroke(&path, 0.55 * u, with_alpha(ink, 0.5));
                }
            }
            if major && len > 260.0 * u {
                index.push((level, pts));
            }
        }
        level += interval;
    }
    // a few depth contours at sea
    let mut d = 10.0f32;
    while d < c.dmax {
        for l in contours(&h, c.inp.sea - d) {
            let pts: Vec<Vec2> = l.pts.iter().map(|p| *p * c.k).collect();
            if l.length() * c.k < 20.0 * u {
                continue;
            }
            if let Some(path) = smooth_path(&pts, l.closed) {
                cv.stroke(&path, 0.5 * u, super::canvas::rgba(70, 120, 170, 90));
            }
        }
        d *= 2.0;
    }
    index
}

// ------------------------------------------------------------------ parchment symbols

const INK_FILL_LIGHT: Rgba = super::canvas::rgb(242, 232, 206);

/// Hatched mountain peaks along the highest ridges, a few hills over the uplands (drawn north to
/// south so nearer symbols overlap the ones behind). Thresholds follow the world's own
/// elevation spread, so a low island still gets its peaks and a continent isn't carpeted.
fn mountains(u: f32, c: &Ctx, keep: &mut Keepout) -> Vec<Symbol> {
    let w = c.w as f32;
    let sea = c.inp.sea;
    let mut land: Vec<f32> = c
        .inp
        .heights
        .v
        .iter()
        .step_by(5)
        .filter(|h| **h > sea + 2.0)
        .map(|h| h - sea)
        .collect();
    if land.len() < 16 {
        return Vec::new();
    }
    land.sort_by(|a, b| a.total_cmp(b));
    let pct = |q: f32| land[((land.len() - 1) as f32 * q) as usize];
    let (hill_lo, mtn_lo, top) = (pct(0.55), pct(0.82), pct(0.997).max(pct(0.82) + 1.0));
    // ridges stand above their surroundings (~120 m around)
    let mut around = c.inp.heights.clone();
    around.blur(((60.0 / c.inp.cell) as usize).max(1), 2);
    let elev = |p: Vec2| c.height(p) - sea;
    let ridge = |p: Vec2| {
        let g = c.grid(p);
        c.inp.heights.bilinear(g.x, g.y) - around.bilinear(g.x, g.y)
    };
    let tall = |p: Vec2| ((elev(p) - mtn_lo) / (top - mtn_lo)).clamp(0.0, 1.0);
    let size_of = |p: Vec2| 24.0 * u + 26.0 * u * tall(p);
    let peaks: Vec<(Vec2, f32)> = super::field::poisson(
        w,
        w,
        16.0 * u,
        |p| size_of(p) * 0.72,
        |p| {
            let n = (value2(p.x / (50.0 * u), p.y / (50.0 * u), 31) - 0.5) * (top - mtn_lo) * 0.3;
            elev(p) > mtn_lo + n && ridge(p) > -2.0 && !keep.blocked(p)
        },
        7,
    )
    .into_iter()
    .map(|p| (p, size_of(p)))
    .collect();
    for (p, s) in &peaks {
        keep.mark_disc(*p, s * 0.5);
    }
    let hills: Vec<(Vec2, f32)> = super::field::poisson(
        w,
        w,
        24.0 * u,
        |_| 24.0 * u,
        |p| {
            let e = elev(p);
            let n =
                (value2(p.x / (40.0 * u), p.y / (40.0 * u), 37) - 0.5) * (mtn_lo - hill_lo) * 0.5;
            e > hill_lo + n
                && e <= mtn_lo
                && ridge(p) > 0.5
                && !keep.blocked(p)
                && c.forest_at(p) < 0.3
        },
        8,
    )
    .into_iter()
    .map(|p| (p, 13.0 * u + 5.0 * u * value2(p.x, p.y, 3)))
    .collect();
    for (p, s) in &hills {
        keep.mark_disc(*p, s * 0.45);
    }
    let mut out: Vec<Symbol> = peaks
        .into_iter()
        .map(|(p, s)| Symbol::Peak(p, s, tall(p) > 0.8))
        .collect();
    out.extend(hills.into_iter().map(|(p, s)| Symbol::Hill(p, s)));
    out
}

/// A map symbol at its anchor, with its size.
enum Symbol {
    Peak(Vec2, f32, bool),
    Hill(Vec2, f32),
    Tree(Vec2, f32, TreeKind),
}

impl Symbol {
    /// Where it stands on the ground (draw order: north to south).
    fn base_y(&self) -> f32 {
        match self {
            Symbol::Peak(p, s, _) => p.y + s * 0.28,
            Symbol::Hill(p, s) => p.y + s * 0.18,
            Symbol::Tree(p, s, _) => p.y + s * 3.0,
        }
    }
}

/// Distance along `dir` from `o` to the first crossing of the polyline, if any.
fn ray_hit(o: Vec2, dir: Vec2, poly: &[Vec2]) -> Option<f32> {
    let mut best: Option<f32> = None;
    for w in poly.windows(2) {
        let (a, b) = (w[0], w[1]);
        let e = b - a;
        let den = dir.perp_dot(e);
        if den.abs() < 1e-6 {
            continue;
        }
        let t = (a - o).perp_dot(e) / den;
        let s = (a - o).perp_dot(dir) / den;
        if t > 1e-3 && (0.0..=1.0).contains(&s) && best.is_none_or(|b| t < b) {
            best = Some(t);
        }
    }
    best
}

/// One mountain: an irregular silhouette (sometimes with a shoulder peak), a paper body that
/// hides the symbols behind, and the shadowed east flank washed and hatched down the slope.
fn mountain_glyph(cv: &mut Canvas, centre: Vec2, size: f32, ink: Rgba, rng: &mut Rng, snowy: bool) {
    let u = cv.u;
    let lw = |x: f32| (x * u).max(0.75);
    let w = size * rng.range(0.85, 1.2);
    let h = size * rng.range(0.55, 0.78);
    let base = centre + Vec2::new(0.0, h * 0.4);
    let peak = base + Vec2::new(rng.range(-0.16, 0.16) * w, -h);
    let l = base + Vec2::new(-w * 0.5, 0.0);
    let r = base + Vec2::new(w * 0.5, 0.0);
    let jit = |rng: &mut Rng, a: Vec2, b: Vec2, t: f32, bulge: f32| {
        a.lerp(b, t) + (b - a).perp().normalize_or_zero() * bulge * w * rng.range(0.4, 1.0)
    };
    // west flank up to the peak, east flank down, a shoulder on one side now and then
    let mut left = vec![l, jit(rng, l, peak, 0.35, -0.035)];
    let shoulder_left = rng.f32() < 0.35;
    if shoulder_left {
        left.push(l.lerp(peak, 0.66) + Vec2::new(-w * 0.03, -h * 0.07));
        left.push(l.lerp(peak, 0.78) + Vec2::new(w * 0.03, h * 0.06));
    } else {
        left.push(jit(rng, l, peak, 0.68, -0.03));
    }
    left.push(peak);
    let mut right = vec![peak];
    if !shoulder_left && rng.f32() < 0.45 {
        right.push(peak.lerp(r, 0.28) + Vec2::new(0.0, h * 0.1));
        right.push(peak.lerp(r, 0.4) + Vec2::new(w * 0.02, -h * 0.03));
    }
    right.push(jit(rng, peak, r, 0.68, 0.035));
    right.push(r);
    let mut outline = left.clone();
    outline.extend(right.iter().skip(1));
    if let Some(body) = poly_path(&outline, true) {
        cv.fill(&body, INK_FILL_LIGHT);
    }
    // the ridge splits light (west) from shadow (east)
    let mid = base + Vec2::new(w * rng.range(0.02, 0.16), 0.0);
    let ridge = chaikin(&[peak, jit(rng, peak, mid, 0.5, 0.06), mid], false, 2);
    let mut shade = right.clone();
    shade.extend(ridge.iter().rev().take(ridge.len() - 1));
    if let Some(p) = poly_path(&shade, true) {
        cv.fill(&p, with_alpha(ink, 0.26));
    }
    // hatching: from the east flank down towards the ridge, roughly along the fall line
    let dir = Vec2::new(-0.42, 1.0).normalize();
    let flank_len = ridge_len(&right);
    let n = ((flank_len / (2.0 * u).max(1.6)) as usize).clamp(4, 22);
    for k in 1..n {
        let t = k as f32 / n as f32;
        if snowy && t < 0.3 {
            continue;
        }
        let a = super::field::point_at(&right, flank_len * t).0 + Vec2::new(-0.4 * u, 0.4 * u);
        let stop = ray_hit(a, dir, &ridge)
            .unwrap_or(f32::MAX)
            .min((base.y - a.y) / dir.y);
        if stop.is_finite() && stop > 1.0 {
            cv.line(a, a + dir * stop * 0.92, lw(0.55), with_alpha(ink, 0.8));
        }
    }
    if let Some(p) = smooth_path(&ridge, false) {
        cv.stroke(&p, lw(0.7), ink);
    }
    if let Some(p) = poly_path(&outline, false) {
        cv.stroke(&p, lw(1.1), ink);
    }
}

fn ridge_len(p: &[Vec2]) -> f32 {
    p.windows(2).map(|w| w[0].distance(w[1])).sum()
}

fn hill_glyph(cv: &mut Canvas, centre: Vec2, size: f32, ink: Rgba, rng: &mut Rng) {
    let u = cv.u;
    let w = size;
    let h = size * rng.range(0.3, 0.4);
    let l = centre + Vec2::new(-w * 0.5, h * 0.5);
    let r = centre + Vec2::new(w * 0.5, h * 0.5);
    let top = centre + Vec2::new(rng.range(-0.12, 0.12) * w, -h * 1.3);
    let mut pb = PathBuilder::new();
    pb.move_to(l.x, l.y);
    pb.quad_to(top.x, top.y, r.x, r.y);
    if let Some(p) = pb.finish() {
        cv.stroke(&p, 0.85 * u, ink);
    }
    // short hatches under the shaded side
    for k in 0..3 {
        let t = 0.6 + k as f32 * 0.12;
        let a = quad_point(l, top, r, t);
        cv.line(
            a + Vec2::new(-w * 0.01, h * 0.2),
            a + Vec2::new(-w * 0.07, h * 0.62),
            0.5 * u,
            with_alpha(ink, 0.6),
        );
    }
}

fn quad_point(a: Vec2, b: Vec2, c: Vec2, t: f32) -> Vec2 {
    let ab = a.lerp(b, t);
    let bc = b.lerp(c, t);
    ab.lerp(bc, t)
}

/// Massed tree symbols wherever the world has woods.
fn forests(u: f32, c: &Ctx, keep: &Keepout) -> Vec<Symbol> {
    let w = c.w as f32;
    let trees = super::field::poisson(
        w,
        w,
        4.9 * u,
        |_| 4.9 * u,
        |p| {
            let f = c.forest_at(p);
            let n = value2(p.x / (9.0 * u), p.y / (9.0 * u), 41) * 0.16;
            f > 0.42 + n && !keep.blocked(p)
        },
        9,
    );
    let mut rng = Rng::new(23);
    trees
        .into_iter()
        .map(|p| Symbol::Tree(p, u * rng.range(0.85, 1.15), c.forest_kind_at(p)))
        .collect()
}

fn tree_glyph(cv: &mut Canvas, p: Vec2, s: f32, kind: TreeKind, ink: Rgba, rng: &mut Rng) {
    let leaf = super::canvas::rgb(190, 196, 134);
    let leaf_dark = super::canvas::rgb(146, 156, 98);
    let line = (0.6 * s).max(0.6);
    match kind {
        TreeKind::Broadleaf => {
            let r = 3.3 * s;
            let c = p + Vec2::new(0.0, -r * 0.55);
            cv.line(
                c + Vec2::new(0.0, r * 0.6),
                c + Vec2::new(0.0, r * 1.45),
                line,
                ink,
            );
            let phase = rng.f32() * 6.0;
            let blob = |cv: &mut Canvas, centre: Vec2, rad: f32, col: Rgba, stroke: bool| {
                let pts: Vec<Vec2> = (0..14)
                    .map(|k| {
                        let a = k as f32 / 14.0 * std::f32::consts::TAU;
                        let rr = rad
                            * (1.0
                                + 0.09 * (3.0 * a + phase).cos()
                                + 0.05 * (5.0 * a + phase * 1.7).sin());
                        centre + Vec2::new(a.cos(), a.sin() * 0.92) * rr
                    })
                    .collect();
                if let Some(path) = smooth_path(&pts, true) {
                    cv.fill(&path, col);
                    if stroke {
                        cv.stroke(&path, line, ink);
                    }
                }
            };
            blob(cv, c, r, leaf_dark, false);
            blob(cv, c + Vec2::new(-0.22, -0.24) * r, r * 0.74, leaf, false);
            // outline last so overlapping crowns read as separate trees
            let pts: Vec<Vec2> = (0..14)
                .map(|k| {
                    let a = k as f32 / 14.0 * std::f32::consts::TAU;
                    let rr = r
                        * (1.0
                            + 0.09 * (3.0 * a + phase).cos()
                            + 0.05 * (5.0 * a + phase * 1.7).sin());
                    c + Vec2::new(a.cos(), a.sin() * 0.92) * rr
                })
                .collect();
            if let Some(path) = smooth_path(&pts, true) {
                cv.stroke(&path, line, ink);
            }
        }
        TreeKind::Conifer => {
            let h = 8.4 * s;
            let w = 4.6 * s;
            let top = p + Vec2::new(0.0, -h * 0.7);
            let base = p + Vec2::new(0.0, h * 0.3);
            let mut pb = PathBuilder::new();
            pb.move_to(top.x, top.y);
            // two tiers
            let t1 = top.lerp(base, 0.55);
            pb.line_to(t1.x + w * 0.32, t1.y);
            pb.line_to(t1.x + w * 0.18, t1.y);
            pb.line_to(base.x + w * 0.5, base.y);
            pb.line_to(base.x - w * 0.5, base.y);
            pb.line_to(t1.x - w * 0.18, t1.y);
            pb.line_to(t1.x - w * 0.32, t1.y);
            pb.close();
            cv.line(base, base + Vec2::new(0.0, 1.4 * s), line, ink);
            if let Some(path) = pb.finish() {
                cv.fill(&path, leaf);
                // shaded right half
                let mut sh = PathBuilder::new();
                sh.move_to(top.x, top.y);
                sh.line_to(t1.x + w * 0.32, t1.y);
                sh.line_to(t1.x + w * 0.18, t1.y);
                sh.line_to(base.x + w * 0.5, base.y);
                sh.line_to(base.x, base.y);
                sh.close();
                if let Some(s2) = sh.finish() {
                    cv.fill(&s2, leaf_dark);
                }
                cv.stroke(&path, line, ink);
            }
        }
        TreeKind::Palm => {
            let h = 7.5 * s;
            let top = p + Vec2::new(1.2 * s, -h * 0.65);
            let base = p + Vec2::new(0.0, h * 0.35);
            let mut pb = PathBuilder::new();
            pb.move_to(base.x, base.y);
            pb.quad_to(base.x + 1.6 * s, base.y - h * 0.5, top.x, top.y);
            if let Some(trunk) = pb.finish() {
                cv.stroke(&trunk, line * 1.2, ink);
            }
            for k in 0..5 {
                let a = -2.6 + k as f32 * 0.55 + rng.range(-0.1, 0.1);
                let tip = top + Vec2::new(a.cos(), a.sin() * 0.7 + 0.35) * 4.2 * s;
                let ctrl = top + Vec2::new(a.cos(), a.sin() - 0.4) * 2.8 * s;
                let mut f = PathBuilder::new();
                f.move_to(top.x, top.y);
                f.quad_to(ctrl.x, ctrl.y, tip.x, tip.y);
                if let Some(path) = f.finish() {
                    cv.stroke(&path, line * 1.6, leaf_dark);
                    cv.stroke(&path, line * 0.7, ink);
                }
            }
        }
        TreeKind::Dead => {
            let h = 7.0 * s;
            let base = p + Vec2::new(0.0, h * 0.35);
            let top = base + Vec2::new(0.0, -h);
            cv.line(base, top, line * 1.2, ink);
            for (t, dx) in [(0.45, -2.2), (0.6, 2.0), (0.78, -1.6), (0.85, 1.4)] {
                let a = base.lerp(top, t);
                cv.line(a, a + Vec2::new(dx * s, -2.0 * s), line, ink);
            }
        }
    }
}

// ------------------------------------------------------------------ roads, buildings, zones

pub fn roads(cv: &mut Canvas, c: &Ctx) {
    let u = cv.u;
    let kit = &c.kit;
    // minor roads first so highways cross over them
    let mut order: Vec<&super::snapshot::RoadInfo> = c.inp.roads.iter().collect();
    order.sort_by_key(|r| RoadKind::ALL.iter().position(|k| *k == r.kind).unwrap_or(0));
    for r in order {
        let pts: Vec<Vec2> = r.line.iter().map(|p| c.world_to_px(*p)).collect();
        let Some(path) = smooth_path(&super::field::simplify(&pts, 0.3 * u), false) else {
            continue;
        };
        let major = matches!(r.kind, RoadKind::Cobblestone | RoadKind::PavedStone);
        let minor = matches!(r.kind, RoadKind::Trail | RoadKind::Footpath);
        match kit.style {
            MapStyle::Parchment => {
                if major {
                    cv.stroke(&path, 2.6 * u, with_alpha(kit.road, 0.9));
                    cv.stroke(&path, 1.0 * u, super::canvas::rgb(238, 224, 190));
                } else if minor {
                    cv.dashed(&path, 1.0 * u, kit.road, &[2.2 * u, 2.6 * u], true);
                } else {
                    cv.dashed(&path, 1.5 * u, kit.road, &[6.0 * u, 3.5 * u], false);
                }
            }
            MapStyle::Atlas => {
                if minor {
                    cv.dashed(
                        &path,
                        1.0 * u,
                        super::canvas::rgb(120, 78, 50),
                        &[3.0 * u, 2.0 * u],
                        false,
                    );
                    continue;
                }
                let (wdt, fill) = if major {
                    (2.8 * u, kit.road)
                } else {
                    (2.0 * u, super::canvas::rgb(255, 252, 240))
                };
                if let Some(cs) = kit.road_casing {
                    cv.stroke(&path, wdt + 1.4 * u, cs);
                }
                cv.stroke(&path, wdt, fill);
            }
            MapStyle::Minimal | MapStyle::Satellite => {
                let wdt = if major {
                    2.4 * u
                } else if minor {
                    1.1 * u
                } else {
                    1.7 * u
                };
                if let Some(cs) = kit.road_casing {
                    cv.stroke(&path, wdt + 1.2 * u, cs);
                }
                if minor {
                    cv.dashed(&path, wdt, kit.road, &[3.0 * u, 2.0 * u], false);
                } else {
                    cv.stroke(&path, wdt, kit.road);
                }
            }
        }
    }
}

pub fn buildings(cv: &mut Canvas, c: &Ctx) {
    if c.kit.style == MapStyle::Satellite {
        return;
    }
    for b in &c.inp.buildings {
        let half = b.radius * 0.62 * c.ppm();
        if half < 0.6 {
            continue;
        }
        let p = c.world_to_px(b.pos);
        let (s, co) = (-b.yaw).sin_cos();
        let corners: Vec<Vec2> = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
            .iter()
            .map(|(x, y)| p + Vec2::new(x * co - y * s, x * s + y * co) * half)
            .collect();
        if let Some(path) = poly_path(&corners, true) {
            cv.fill(&path, c.kit.building);
        }
    }
}

/// Zone borders as polylines in map px, one per stretch between two zones (or a zone and the
/// wilderness), each drawn once; only over land.
pub struct ZoneLine {
    pub a: u8,
    pub b: u8,
    pub pts: Vec<Vec2>,
}

pub fn zone_borders(c: &Ctx) -> Vec<ZoneLine> {
    let n = c.inp.n();
    let z = &c.inp.zone_ids;
    let h = &c.inp.heights.v;
    let shown = |id: u8| c.inp.zones.iter().any(|zz| zz.id == id);
    // dual-grid corner (i + 0.5, j + 0.5) for i, j in -1..n
    let key = |i: isize, j: isize| ((j + 1) as usize) * (n + 1) + (i + 1) as usize;
    let mut points: HashMap<usize, Vec2> = HashMap::new();
    let mut per_pair: HashMap<(u8, u8), Vec<(usize, usize)>> = HashMap::new();
    let mut add =
        |a: u8, b: u8, p: (isize, isize), q: (isize, isize), points: &mut HashMap<usize, Vec2>| {
            let (ka, kb) = (key(p.0, p.1), key(q.0, q.1));
            points
                .entry(ka)
                .or_insert(Vec2::new(p.0 as f32 + 0.5, p.1 as f32 + 0.5));
            points
                .entry(kb)
                .or_insert(Vec2::new(q.0 as f32 + 0.5, q.1 as f32 + 0.5));
            per_pair
                .entry((a.min(b), a.max(b)))
                .or_default()
                .push((ka, kb));
        };
    for j in 0..n {
        for i in 0..n {
            let a = z[j * n + i];
            // right neighbour → vertical border segment; lower neighbour → horizontal one
            if i + 1 < n {
                let b = z[j * n + i + 1];
                if a != b
                    && (shown(a) || shown(b))
                    && (h[j * n + i] > c.inp.sea || h[j * n + i + 1] > c.inp.sea)
                {
                    add(
                        a,
                        b,
                        (i as isize, j as isize - 1),
                        (i as isize, j as isize),
                        &mut points,
                    );
                }
            }
            if j + 1 < n {
                let b = z[(j + 1) * n + i];
                if a != b
                    && (shown(a) || shown(b))
                    && (h[j * n + i] > c.inp.sea || h[(j + 1) * n + i] > c.inp.sea)
                {
                    add(
                        a,
                        b,
                        (i as isize - 1, j as isize),
                        (i as isize, j as isize),
                        &mut points,
                    );
                }
            }
        }
    }
    let mut out = Vec::new();
    for ((a, b), segs) in per_pair {
        for l in chain(&segs, &points) {
            if l.pts.len() < 4 {
                continue;
            }
            // staircase → smooth: moving average, then corner cutting
            let sm = moving_average(&l, 3);
            let pts: Vec<Vec2> = chaikin(&sm, l.closed, 2)
                .into_iter()
                .map(|p| p * c.k)
                .collect();
            let mut pts = pts;
            if l.closed
                && let Some(f) = pts.first().copied()
            {
                pts.push(f);
            }
            out.push(ZoneLine { a, b, pts });
        }
    }
    out
}

fn moving_average(l: &Line, r: usize) -> Vec<Vec2> {
    let n = l.pts.len();
    (0..n)
        .map(|i| {
            if !l.closed && (i < r || i + r >= n) {
                // ends stay put (junctions between three zones)
                let lo = i.saturating_sub(i.min(r));
                let hi = (i + (n - 1 - i).min(r)).min(n - 1);
                let k = (i - lo).min(hi - i);
                let (a, b) = (i - k, i + k);
                return l.pts[a..=b].iter().copied().sum::<Vec2>() / (b - a + 1) as f32;
            }
            let mut s = Vec2::ZERO;
            for d in 0..=2 * r {
                let k = (i + n + d - r) % n;
                s += l.pts[k];
            }
            s / (2 * r + 1) as f32
        })
        .collect()
}

pub fn draw_zone_borders(cv: &mut Canvas, c: &Ctx, lines: &[ZoneLine]) {
    let u = cv.u;
    let kit = &c.kit;
    for l in lines {
        let Some(path) = smooth_path(&l.pts, false) else {
            continue;
        };
        if let Some(sh) = kit.border_shadow {
            cv.stroke(&path, 3.2 * u, sh);
        }
        let dash = match kit.style {
            MapStyle::Parchment => vec![9.0 * u, 3.0 * u, 2.0 * u, 3.0 * u],
            _ => vec![7.0 * u, 4.0 * u],
        };
        cv.dashed(&path, 1.35 * u, kit.border, &dash, false);
    }
}

/// Zone outlines as closed polygons in map px (for the game's map data).
pub fn zone_polygons(c: &Ctx, id: u8) -> Vec<Vec<Vec2>> {
    let n = c.inp.n();
    let g = Grid::from_vec(
        n,
        n,
        c.inp
            .zone_ids
            .iter()
            .map(|z| if *z == id { 1.0 } else { 0.0 })
            .collect(),
    );
    contours(&g, 0.5)
        .into_iter()
        .filter(|l| l.pts.len() >= 4)
        .map(|l| {
            let pts: Vec<Vec2> = l.pts.iter().map(|p| *p * c.k).collect();
            let mut s = super::field::simplify(&pts, 1.5 * c.w as f32 / 2048.0);
            if l.closed && s.len() > 2 && s.first() == s.last() {
                s.pop();
            }
            s
        })
        .filter(|p| p.len() >= 3)
        .collect()
}
