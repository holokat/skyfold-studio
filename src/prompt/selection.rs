//! The area selection: a lasso or polygon drawn on the ground from any tab (the top bar's
//! sparkle button or G), shown as a glowing outline with a soft tint, and the feathered weight
//! raster (`MaskField`) that keeps edits inside it (see `mask`).
//!
//! Drawing: drag to lasso freehand (release finishes), or click points (Enter, a double-click or
//! a click on the first point finishes; Backspace removes the last point). Esc cancels.

use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, Id, LayerId, Order, Pos2, Stroke};

use crate::camera::{MainCamera, Pointer};
use crate::noise::{fbm, smoothstep};
use crate::terrain::overlay::{TerrainOverlay, new_overlay_image};
use crate::terrain::{GridRect, Terrain};

pub const OVERLAY_OWNER: &str = "selection";
/// Hotkey of the selection tool (free in every tab and in walk mode's bindings).
pub const HOTKEY: KeyCode = KeyCode::KeyG;
const OUTLINE: Color32 = Color32::WHITE;

/// Feathered weights (0 outside … 1 well inside) on the terrain grid around a polygon.
#[derive(Clone)]
pub struct MaskField {
    pub poly: Vec<Vec2>,
    /// Width of the soft edge (m).
    pub feather: f32,
    /// Grid rect the weights cover (vertex coordinates); everything outside is 0.
    pub rect: GridRect,
    weights: Vec<f32>,
    /// Terrain layout the raster was made for: (generation, res, cell).
    layout: (u64, usize, f32),
    half: f32,
}

impl MaskField {
    /// `edge_noise` (m) roughens the outline organically. `inward_only`: the noise only eats into
    /// the polygon (never past it), as the selection's own mask must.
    pub fn build(
        t: &Terrain,
        poly: &[Vec2],
        feather: f32,
        edge_noise: f32,
        inward_only: bool,
        seed: u32,
    ) -> Option<MaskField> {
        let (lo, hi) = bounds(poly);
        let pad = if inward_only { 0.0 } else { edge_noise };
        let g0 = t.world_to_grid(lo.x - pad, lo.y - pad);
        let g1 = t.world_to_grid(hi.x + pad, hi.y + pad);
        let max = t.res as f32;
        if g1.x < 0.0 || g1.y < 0.0 || g0.x > max || g0.y > max {
            return None;
        }
        let rect = GridRect {
            x0: g0.x.floor().clamp(0.0, max) as usize,
            z0: g0.y.floor().clamp(0.0, max) as usize,
            x1: g1.x.ceil().clamp(0.0, max) as usize,
            z1: g1.y.ceil().clamp(0.0, max) as usize,
        };
        let origin = t.grid_to_world(rect.x0, rect.z0);
        let band = feather + edge_noise + t.cell;
        let sd = signed_distance(poly, origin, t.cell, rect.width(), rect.height(), band);
        let freq = 1.0 / (edge_noise * 2.5).max(8.0);
        let weights = sd
            .iter()
            .enumerate()
            .map(|(k, d)| {
                let mut d = *d;
                if edge_noise > 0.0 && d.abs() < band {
                    let p = origin
                        + Vec2::new((k % rect.width()) as f32, (k / rect.width()) as f32) * t.cell;
                    // (fbm rarely leaves ±0.45: stretched to use the whole amplitude)
                    let n = (fbm(p.x * freq, p.y * freq, 3, 2.0, 0.5, seed) * 2.2).clamp(-1.0, 1.0);
                    d += if inward_only {
                        -(n * 0.5 + 0.5) * edge_noise
                    } else {
                        n * edge_noise
                    };
                }
                if d <= 0.0 {
                    0.0
                } else {
                    smoothstep(0.0, feather.max(0.01), d)
                }
            })
            .collect();
        Some(MaskField {
            poly: poly.to_vec(),
            feather,
            rect,
            weights,
            layout: (t.generation, t.res, t.cell),
            half: t.half(),
        })
    }

    /// Weight at grid vertex (i, j).
    #[inline]
    pub fn at_grid(&self, i: usize, j: usize) -> f32 {
        let r = &self.rect;
        if i < r.x0 || i > r.x1 || j < r.z0 || j > r.z1 {
            return 0.0;
        }
        self.weights[(j - r.z0) * r.width() + (i - r.x0)]
    }

    /// Weight at a world position (bilinear).
    pub fn at(&self, p: Vec2) -> f32 {
        let g = (p + Vec2::splat(self.half)) / self.layout.2;
        if g.x < 0.0 || g.y < 0.0 {
            return 0.0;
        }
        let (i, j) = (g.x.floor() as usize, g.y.floor() as usize);
        let (fx, fz) = (g.x.fract(), g.y.fract());
        let a = self.at_grid(i, j) + (self.at_grid(i + 1, j) - self.at_grid(i, j)) * fx;
        let b = self.at_grid(i, j + 1) + (self.at_grid(i + 1, j + 1) - self.at_grid(i, j + 1)) * fx;
        a + (b - a) * fz
    }

    /// How (rect ∩ tile) relates to the field: Some(true) all weights 1, Some(false) all 0.
    pub fn uniform_in(&self, r: GridRect) -> Option<bool> {
        let f = &self.rect;
        if r.x1 < f.x0 || r.x0 > f.x1 || r.z1 < f.z0 || r.z0 > f.z1 {
            return Some(false);
        }
        if r.x0 < f.x0 || r.x1 > f.x1 || r.z0 < f.z0 || r.z1 > f.z1 {
            return None;
        }
        let first = self.at_grid(r.x0, r.z0);
        if first != 0.0 && first != 1.0 {
            return None;
        }
        for j in r.z0..=r.z1 {
            for i in r.x0..=r.x1 {
                if self.at_grid(i, j) != first {
                    return None;
                }
            }
        }
        Some(first == 1.0)
    }

    fn fits(&self, t: &Terrain) -> bool {
        self.layout == (t.generation, t.res, t.cell)
    }
}

pub fn bounds(poly: &[Vec2]) -> (Vec2, Vec2) {
    poly.iter()
        .fold((Vec2::MAX, Vec2::MIN), |(a, b), p| (a.min(*p), b.max(*p)))
}

pub fn area(poly: &[Vec2]) -> f32 {
    let mut s = 0.0;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        s += a.x * b.y - b.x * a.y;
    }
    (s * 0.5).abs()
}

pub fn perimeter(poly: &[Vec2]) -> f32 {
    (0..poly.len())
        .map(|i| poly[i].distance(poly[(i + 1) % poly.len()]))
        .sum()
}

/// The default soft edge for a selection: about 8% of its size, 15–60 m.
pub fn auto_feather(poly: &[Vec2]) -> f32 {
    let (lo, hi) = bounds(poly);
    ((hi - lo).max_element() * 0.08).clamp(15.0, 60.0)
}

/// Signed distance to a closed polygon (positive inside), clamped to ±`band`, on a `w` × `h`
/// grid from `origin` with spacing `step`.
pub fn signed_distance(
    poly: &[Vec2],
    origin: Vec2,
    step: f32,
    w: usize,
    h: usize,
    band: f32,
) -> Vec<f32> {
    let mut d = vec![band; w * h];
    // distance near the edges: each segment updates the cells within `band` of it
    let n = poly.len();
    for e in 0..n {
        let (a, b) = (poly[e], poly[(e + 1) % n]);
        let (lo, hi) = (a.min(b) - Vec2::splat(band), a.max(b) + Vec2::splat(band));
        let i0 = ((lo.x - origin.x) / step).floor().max(0.0) as usize;
        let j0 = ((lo.y - origin.y) / step).floor().max(0.0) as usize;
        let i1 = (((hi.x - origin.x) / step).ceil().max(0.0) as usize).min(w.saturating_sub(1));
        let j1 = (((hi.y - origin.y) / step).ceil().max(0.0) as usize).min(h.saturating_sub(1));
        if i0 > i1 || j0 > j1 {
            continue;
        }
        let ab = b - a;
        let len2 = ab.length_squared().max(1e-6);
        for j in j0..=j1 {
            for i in i0..=i1 {
                let p = origin + Vec2::new(i as f32, j as f32) * step;
                let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
                let dist = p.distance(a + ab * t);
                let k = j * w + i;
                if dist < d[k] {
                    d[k] = dist;
                }
            }
        }
    }
    // inside: even-odd crossings along each row
    let mut xs: Vec<f32> = Vec::with_capacity(16);
    for j in 0..h {
        let z = origin.y + j as f32 * step;
        xs.clear();
        for e in 0..n {
            let (a, b) = (poly[e], poly[(e + 1) % n]);
            if (a.y > z) != (b.y > z) {
                xs.push(a.x + (z - a.y) * (b.x - a.x) / (b.y - a.y));
            }
        }
        xs.sort_by(|a, b| a.total_cmp(b));
        let row = &mut d[j * w..(j + 1) * w];
        for (i, v) in row.iter_mut().enumerate() {
            let x = origin.x + i as f32 * step;
            let inside = xs.iter().filter(|c| **c < x).count() % 2 == 1;
            if !inside {
                *v = -*v;
            }
        }
    }
    d
}

/// The lasso being drawn.
#[derive(Default)]
struct Draft {
    points: Vec<Vec2>,
    /// Mouse down: (screen position, whether it started a freehand lasso, moved yet).
    press: Option<(Vec2, bool, bool)>,
    /// Last click (real time s, screen position) for double-clicks.
    last_click: Option<(f32, Vec2)>,
}

#[derive(Resource)]
pub struct Selection {
    polygon: Option<Vec<Vec2>>,
    /// The "Mask edits to selection" toggle.
    pub mask_on: bool,
    /// Agent runs in progress (they always mask).
    pub forced: u32,
    /// The selection tool is drawing.
    pub drawing: bool,
    draft: Draft,
    field: Option<MaskField>,
    /// Bumped whenever the polygon changes.
    pub revision: u64,
    overlay: Option<(u64, u64, Handle<Image>, Handle<Image>)>,
    /// A finished lasso, for the prompt panel to pick up.
    pub just_selected: bool,
    /// Screenshots that want the outline drawn on the ground.
    pub shot_outline: u32,
    /// The terrain the selection was made on (a new or loaded world clears it).
    generation: Option<u64>,
    /// An underground dungeon is open: the floor of the level being edited (m), where the lasso
    /// and its outline lie (`dungeon_floor`).
    pub dungeon_floor: Option<f32>,
}

impl Default for Selection {
    fn default() -> Self {
        Self {
            polygon: None,
            mask_on: true,
            forced: 0,
            drawing: false,
            draft: Draft::default(),
            field: None,
            revision: 0,
            overlay: None,
            just_selected: false,
            shot_outline: 0,
            generation: None,
            dungeon_floor: None,
        }
    }
}

impl Selection {
    pub fn polygon(&self) -> Option<&[Vec2]> {
        self.polygon.as_deref()
    }

    /// Sets the selected area (cleaned up: duplicate points dropped, points spaced at least
    /// `min_step` m, clamped to the world). Errors if it is degenerate.
    pub fn set(&mut self, t: &Terrain, poly: &[Vec2]) -> Result<(), String> {
        // (in a dungeon a single small room is an area too)
        let min_area = if self.dungeon_floor.is_some() {
            16.0
        } else {
            400.0
        };
        let h = t.half() - 0.5;
        let per = perimeter(poly);
        let min_step = (t.cell * 0.75).max(per / 512.0);
        let mut clean: Vec<Vec2> = vec![];
        for p in poly {
            let p = p.clamp(Vec2::splat(-h), Vec2::splat(h));
            if clean
                .last()
                .is_none_or(|l: &Vec2| l.distance(p) >= min_step)
            {
                clean.push(p);
            }
        }
        while clean.len() > 3 && clean[0].distance(clean[clean.len() - 1]) < min_step {
            clean.pop();
        }
        if clean.len() < 3 || area(&clean) < min_area {
            return Err(format!(
                "the area must be a polygon of at least 3 points covering at least {min_area:.0} m²"
            ));
        }
        self.polygon = Some(clean);
        self.field = None;
        self.revision += 1;
        Ok(())
    }

    pub fn clear(&mut self) {
        self.polygon = None;
        self.field = None;
        self.revision += 1;
    }

    /// The selection's weight raster (whether or not it masks edits).
    pub fn field(&mut self, t: &Terrain) -> Option<&MaskField> {
        let poly = self.polygon.as_deref()?;
        if self.field.as_ref().is_none_or(|f| !f.fits(t)) {
            // edits fade out along an organic line inside the outline (not a copy of the lasso)
            let feather = auto_feather(poly);
            self.field = MaskField::build(t, poly, feather, feather * 1.3, true, 9173);
        }
        self.field.as_ref()
    }

    /// Edits are masked to the selection right now.
    pub fn masking(&self) -> bool {
        self.polygon.is_some() && (self.mask_on || self.forced > 0)
    }

    /// The edit mask, when masking.
    pub fn mask(&mut self, t: &Terrain) -> Option<&MaskField> {
        if !self.masking() {
            return None;
        }
        self.field(t)
    }

    /// Corners (or lasso points) drawn so far.
    pub fn draft_len(&self) -> usize {
        self.draft.points.len()
    }

    pub fn start_drawing(&mut self) {
        self.drawing = true;
        self.draft = Draft::default();
    }

    pub fn cancel_drawing(&mut self) {
        self.drawing = false;
        self.draft = Draft::default();
    }
}

/// The selection tool's mouse and keys. Runs before the tools see the click (it takes the left
/// button's press while drawing, so no brush stroke starts).
pub(crate) fn lasso_input(
    mut sel: ResMut<Selection>,
    pointer: Res<Pointer>,
    mut buttons: ResMut<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time<Real>>,
    walk: Res<crate::walk::WalkState>,
    terrain: Res<Terrain>,
    mut ed: ResMut<crate::editor::Editor>,
) {
    let cmd = keys.any_pressed([
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
    ]);
    if walk.active {
        if sel.drawing {
            sel.cancel_drawing();
        }
        return;
    }
    if !pointer.keyboard_captured && !cmd && keys.just_pressed(HOTKEY) {
        if sel.drawing {
            sel.cancel_drawing();
        } else {
            sel.start_drawing();
            ed.toast("Select an area: drag a lasso, or click points (Enter or double-click to finish, Esc to cancel)");
        }
    }
    if !sel.drawing {
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        sel.cancel_drawing();
        ed.toast("Selection cancelled");
        return;
    }
    let now = time.elapsed_secs();
    let mut finish = !pointer.keyboard_captured
        && (keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter));
    if !pointer.keyboard_captured && keys.just_pressed(KeyCode::Backspace) {
        sel.draft.points.pop();
    }
    let space = keys.pressed(KeyCode::Space);
    let screen = pointer.screen.unwrap_or_default();
    if buttons.just_pressed(MouseButton::Left) && !pointer.over_ui && !space {
        // ours: no brush stroke or click for the tab's tool
        buttons.clear_just_pressed(MouseButton::Left);
        let fresh = sel.draft.points.is_empty();
        sel.draft.press = Some((screen, fresh, false));
    }
    let step = if sel.dungeon_floor.is_some() {
        1.0
    } else {
        (terrain.cell * 1.5).max(3.0)
    };
    if let Some((start, fresh, moved)) = sel.draft.press {
        let moved = moved || screen.distance(start) > 6.0;
        sel.draft.press = Some((start, fresh, moved));
        if moved
            && fresh
            && buttons.pressed(MouseButton::Left)
            && let Some(hit) = pointer.hit
            && sel
                .draft
                .points
                .last()
                .is_none_or(|l| l.distance(hit.xz()) >= step)
        {
            sel.draft.points.push(hit.xz());
        }
        if !buttons.pressed(MouseButton::Left) {
            sel.draft.press = None;
            if moved && fresh {
                finish = true;
            } else if !moved {
                // a click: double-click or a click on the first point closes the polygon
                let double = sel
                    .draft
                    .last_click
                    .is_some_and(|(t, p)| now - t < 0.4 && p.distance(screen) < 8.0);
                sel.draft.last_click = Some((now, screen));
                if double
                    || (sel.draft.points.len() >= 3
                        && pointer
                            .hit
                            .is_some_and(|h| near_first(&sel.draft.points, h.xz(), step)))
                {
                    finish = true;
                } else if let Some(hit) = pointer.hit {
                    sel.draft.points.push(hit.xz());
                }
            }
        }
    }
    if finish {
        let pts = std::mem::take(&mut sel.draft.points);
        sel.cancel_drawing();
        match sel.set(&terrain, &pts) {
            Ok(()) => {
                sel.just_selected = true;
                let a = area(sel.polygon().unwrap_or_default());
                ed.toast(format!(
                    "Area selected ({}). Describe what you want there.",
                    area_text(a)
                ));
            }
            Err(_) if pts.len() < 3 => {
                ed.toast("Selection needs at least 3 points: keep clicking, or drag a lasso")
            }
            Err(e) => ed.toast(format!("Selection too small: {e}")),
        }
    }
}

fn near_first(points: &[Vec2], p: Vec2, step: f32) -> bool {
    points
        .first()
        .is_some_and(|f| f.distance(p) < (step * 2.5).max(perimeter(points) * 0.03))
}

/// Keeps `Selection::dungeon_floor` on the edited level's floor while an underground dungeon is
/// open (entering or leaving one loads another map, which clears the selection).
pub(crate) fn dungeon_floor(
    mut sel: ResMut<Selection>,
    map: Res<crate::dungeons::DungeonMap>,
    view: Res<crate::dungeons::render::DungeonView>,
) {
    let f = map
        .0
        .as_ref()
        .filter(|_| map.interior())
        .map(|m| m.plan.level_y(view.level));
    if sel.dungeon_floor != f {
        sel.dungeon_floor = f;
    }
}

/// "12.5 ha" / "3.2 km²".
pub fn area_text(m2: f32) -> String {
    if m2 >= 1.0e6 {
        format!("{:.1} km²", m2 / 1.0e6)
    } else {
        format!("{:.1} ha", m2 / 1.0e4)
    }
}

/// Shows the soft tint and edge on the ground (the edge alone in screenshots that ask for it).
pub(crate) fn update_overlay(
    mut sel: ResMut<Selection>,
    terrain: Res<Terrain>,
    mut overlay: ResMut<TerrainOverlay>,
    mut images: ResMut<Assets<Image>>,
    mcp: Option<Res<crate::mcp::McpServer>>,
) {
    let clean = !crate::mcp::ui_visible(mcp);
    if sel.generation != Some(terrain.generation) {
        if sel.generation.is_some() {
            sel.clear();
        }
        sel.generation = Some(terrain.generation);
    }
    let Some(poly) = sel.polygon.clone() else {
        overlay.clear(OVERLAY_OWNER);
        sel.overlay = None;
        return;
    };
    // another tool's overlay (an analysis heatmap) has the ground while its tab is open
    if overlay.image.is_some() && overlay.owner != OVERLAY_OWNER {
        return;
    }
    let key = (sel.revision, terrain.generation);
    if sel.overlay.as_ref().is_none_or(|o| (o.0, o.1) != key) {
        let (full, edge) = overlay_images(&terrain, &poly);
        sel.overlay = Some((key.0, key.1, images.add(full), images.add(edge)));
    }
    let Some((_, _, full, edge)) = &sel.overlay else {
        return;
    };
    if clean && sel.shot_outline == 0 {
        overlay.clear(OVERLAY_OWNER);
    } else {
        let want = if clean { edge.clone() } else { full.clone() };
        if overlay.image.as_ref() != Some(&want) {
            overlay.show(OVERLAY_OWNER, want, 1.0);
        }
    }
}

/// (tint + glowing edge, edge only) images over the whole terrain, ~2 m per texel.
fn overlay_images(t: &Terrain, poly: &[Vec2]) -> (Image, Image) {
    let size = t.size();
    let res = ((size / 2.0) as u32).clamp(512, 4096);
    let texel = size / res as f32;
    let mut full = new_overlay_image(res, res);
    let mut edge = new_overlay_image(res, res);
    let (lo, hi) = bounds(poly);
    let half = t.half();
    let band = texel * 4.0;
    let i0 = (((lo.x - band + half) / texel).floor().max(0.0) as u32).min(res - 1);
    let j0 = (((lo.y - band + half) / texel).floor().max(0.0) as u32).min(res - 1);
    let i1 = (((hi.x + band + half) / texel).ceil().max(0.0) as u32).min(res - 1);
    let j1 = (((hi.y + band + half) / texel).ceil().max(0.0) as u32).min(res - 1);
    let (w, h) = ((i1 - i0 + 1) as usize, (j1 - j0 + 1) as usize);
    let origin = Vec2::new(
        (i0 as f32 + 0.5) * texel - half,
        (j0 as f32 + 0.5) * texel - half,
    );
    let feather = auto_feather(poly);
    let sd = signed_distance(poly, origin, texel, w, h, feather.max(band));
    let (Some(fd), Some(ed)) = (full.data.as_mut(), edge.data.as_mut()) else {
        return (full, edge);
    };
    for j in 0..h {
        for i in 0..w {
            let d = sd[j * w + i];
            let line = (-(d / (texel * 1.1)).powi(2)).exp();
            let inside = smoothstep(0.0, feather, d);
            let tint = 0.08 + 0.06 * (1.0 - inside);
            let a_full = if d > 0.0 { line.max(tint) } else { line * 0.85 };
            let k = (((j0 as usize + j) * res as usize) + i0 as usize + i) * 4;
            let col = [150u8, 215, 255];
            if a_full > 0.004 {
                fd[k..k + 3].copy_from_slice(&col);
                fd[k + 3] = (a_full.min(1.0) * 255.0) as u8;
            }
            if line > 0.02 {
                ed[k..k + 3].copy_from_slice(&col);
                ed[k + 3] = (line * 0.8 * 255.0) as u8;
            }
        }
    }
    (full, edge)
}

/// Screen-space outline (glow) of the selection, and the lasso while drawing.
pub(crate) fn draw_outline(
    ctx: &egui::Context,
    sel: &Selection,
    t: &Terrain,
    pointer: &Pointer,
    cam: &Camera,
    cam_t: &GlobalTransform,
    now: f32,
) {
    let painter = ctx.layer_painter(LayerId::new(
        Order::Background,
        Id::new("selection_outline"),
    ));
    let project = |p: Vec2| -> Option<Pos2> {
        let y = sel.dungeon_floor.map_or_else(
            || t.height_at(p.x, p.y).max(t.sea_level) + 1.0,
            |f| f + 0.15,
        );
        let v = cam.world_to_viewport(cam_t, Vec3::new(p.x, y, p.y)).ok()?;
        Some(Pos2::new(v.x, v.y))
    };
    let stroke_path = |pts: &[Vec2], closed: bool, alpha: f32| {
        let mut dense = vec![];
        let n = pts.len();
        let segs = if closed { n } else { n.saturating_sub(1) };
        for k in 0..segs {
            let (a, b) = (pts[k], pts[(k + 1) % n]);
            let m = ((a.distance(b)
                / if sel.dungeon_floor.is_some() {
                    1.0
                } else {
                    t.cell * 3.0
                })
            .ceil() as usize)
                .clamp(1, 64);
            for s in 0..m {
                dense.push(project(a.lerp(b, s as f32 / m as f32)));
            }
        }
        if !closed && let Some(l) = pts.last() {
            dense.push(project(*l));
        }
        if closed && let Some(f) = dense.first().cloned() {
            dense.push(f);
        }
        for (width, a) in [(9.0, 0.10), (5.0, 0.22), (2.0, 1.0)] {
            let c = OUTLINE.gamma_multiply(a * alpha);
            for w in dense.windows(2) {
                if let (Some(p), Some(q)) = (w[0], w[1]) {
                    painter.line_segment([p, q], Stroke::new(width, c));
                }
            }
        }
    };
    if let Some(poly) = sel.polygon() {
        let pulse = 0.85 + 0.15 * (now * 2.2).sin();
        stroke_path(poly, true, if sel.drawing { 0.4 } else { pulse });
    }
    if sel.drawing {
        let mut pts = sel.draft.points.clone();
        let clicking = sel.draft.press.is_none_or(|p| !p.1 || !p.2);
        if clicking && let Some(h) = pointer.hit {
            pts.push(h.xz());
        }
        if pts.len() >= 2 {
            stroke_path(&pts, false, 1.0);
        }
        if let Some(f) = sel.draft.points.first().and_then(|f| project(*f)) {
            painter.circle_stroke(f, 6.0, Stroke::new(2.0, OUTLINE));
        }
        if let Some(h) = pointer.hit.and_then(|h| project(h.xz())) {
            painter.circle_filled(h, 3.5, OUTLINE);
        }
    }
}

/// Keeps the camera query small for the panel system.
pub(crate) type CameraQuery<'w, 's> =
    Query<'w, 's, (&'static Camera, &'static GlobalTransform), With<MainCamera>>;
