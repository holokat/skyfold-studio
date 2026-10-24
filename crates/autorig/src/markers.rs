//! Getting a character ready to rig: `normalize` stands it on y = 0 facing +Z in metres, and
//! `guess` finds the eight body markers (chin, groin, wrists, elbows, knees) on its front
//! silhouette, the way a person would click them in Mixamo. Each marker sits in the middle of the
//! body's depth there.
//!
//! The markers are only anchors: `crate::fit` maps the template skeleton from the template's
//! own markers (`TEMPLATE`, measured with these same detectors) onto the character's.

use glam::{Mat4, Quat, Vec2, Vec3};
use serde::{Deserialize, Serialize};

use crate::model::Model;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Markers {
    pub chin: Vec3,
    pub groin: Vec3,
    pub wrist_l: Vec3,
    pub wrist_r: Vec3,
    pub elbow_l: Vec3,
    pub elbow_r: Vec3,
    pub knee_l: Vec3,
    pub knee_r: Vec3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Marker {
    Chin,
    Groin,
    WristL,
    WristR,
    ElbowL,
    ElbowR,
    KneeL,
    KneeR,
}

impl Marker {
    pub const ALL: [Marker; 8] = [
        Marker::Chin,
        Marker::WristL,
        Marker::WristR,
        Marker::ElbowL,
        Marker::ElbowR,
        Marker::Groin,
        Marker::KneeL,
        Marker::KneeR,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Marker::Chin => "Chin",
            Marker::Groin => "Groin",
            Marker::WristL => "Left wrist",
            Marker::WristR => "Right wrist",
            Marker::ElbowL => "Left elbow",
            Marker::ElbowR => "Right elbow",
            Marker::KneeL => "Left knee",
            Marker::KneeR => "Right knee",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Marker::Chin => "chin",
            Marker::Groin => "groin",
            Marker::WristL => "wrist_l",
            Marker::WristR => "wrist_r",
            Marker::ElbowL => "elbow_l",
            Marker::ElbowR => "elbow_r",
            Marker::KneeL => "knee_l",
            Marker::KneeR => "knee_r",
        }
    }
    pub fn from_id(id: &str) -> Option<Marker> {
        Marker::ALL.into_iter().find(|m| m.id() == id)
    }
}

impl Markers {
    pub fn get(&self, m: Marker) -> Vec3 {
        match m {
            Marker::Chin => self.chin,
            Marker::Groin => self.groin,
            Marker::WristL => self.wrist_l,
            Marker::WristR => self.wrist_r,
            Marker::ElbowL => self.elbow_l,
            Marker::ElbowR => self.elbow_r,
            Marker::KneeL => self.knee_l,
            Marker::KneeR => self.knee_r,
        }
    }
    pub fn set(&mut self, m: Marker, v: Vec3) {
        *match m {
            Marker::Chin => &mut self.chin,
            Marker::Groin => &mut self.groin,
            Marker::WristL => &mut self.wrist_l,
            Marker::WristR => &mut self.wrist_r,
            Marker::ElbowL => &mut self.elbow_l,
            Marker::ElbowR => &mut self.elbow_r,
            Marker::KneeL => &mut self.knee_l,
            Marker::KneeR => &mut self.knee_r,
        } = v;
    }
}

/// The template mannequin's markers, found by `guess` on its own mesh (`tests::calibrate`).
pub const TEMPLATE: Markers = Markers {
    chin: Vec3::new(0.0, 1.5803, -0.0283),
    groin: Vec3::new(0.0, 0.8382, -0.0249),
    wrist_l: Vec3::new(0.7381, 1.4525, -0.0823),
    wrist_r: Vec3::new(-0.7381, 1.4525, -0.0823),
    elbow_l: Vec3::new(0.4611, 1.4643, -0.0933),
    elbow_r: Vec3::new(-0.4611, 1.4643, -0.0933),
    knee_l: Vec3::new(0.0914, 0.5288, -0.0133),
    knee_r: Vec3::new(-0.0914, 0.5288, -0.0133),
};
/// The template's torso half-width at its chest row (`Silhouette::torso_half_width`).
pub const TEMPLATE_TORSO_HALF: f32 = 0.1524;

/// What `normalize` did.
#[derive(Clone, Copy, Debug)]
pub struct Normalized {
    /// applied to the model (and its skeleton)
    pub transform: Mat4,
    pub height: f32,
    /// quarter turns about +Y that were applied to face +Z
    pub quarter_turns: i32,
}

/// Stands the model on y = 0, centred on x = z = 0, facing +Z, in metres (centimetre and
/// millimetre files are recognised by their size).
pub fn normalize(model: &mut Model, detect_facing: bool) -> Normalized {
    let Some((lo, hi)) = model.bounds() else {
        return Normalized {
            transform: Mat4::IDENTITY,
            height: 0.0,
            quarter_turns: 0,
        };
    };
    let h = hi.y - lo.y;
    let unit = if h > 200.0 {
        0.001
    } else if h > 20.0 {
        0.01
    } else {
        1.0
    };
    let turns = if detect_facing { facing_turns(model) } else { 0 };
    let rot = Quat::from_rotation_y(turns as f32 * std::f32::consts::FRAC_PI_2);
    let pre = Mat4::from_scale(Vec3::splat(unit)) * Mat4::from_quat(rot);
    let (lo2, hi2) = bounds_after(model, pre);
    let centre = Vec3::new((lo2.x + hi2.x) * 0.5, lo2.y, (lo2.z + hi2.z) * 0.5);
    let m = Mat4::from_translation(-centre) * pre;
    model.transform(m);
    Normalized {
        transform: m,
        height: (hi2.y - lo2.y),
        quarter_turns: turns,
    }
}

fn bounds_after(model: &Model, m: Mat4) -> (Vec3, Vec3) {
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    for mesh in &model.meshes {
        for p in &mesh.positions {
            let q = m.transform_point3(*p);
            lo = lo.min(q);
            hi = hi.max(q);
        }
    }
    (lo, hi)
}

/// Quarter turns about +Y that face the character +Z: the arms (wide at shoulder height) give
/// the axis, the toes (the feet reach further forward than the ankles) its sign.
fn facing_turns(model: &Model) -> i32 {
    let Some((lo, hi)) = model.bounds() else {
        return 0;
    };
    let h = hi.y - lo.y;
    let band = |a: f32, b: f32| {
        model
            .meshes
            .iter()
            .flat_map(|m| m.positions.iter())
            .filter(move |p| p.y >= lo.y + a * h && p.y <= lo.y + b * h)
    };
    // the axis the shoulders span
    let (mut sx, mut sz) = (Vec2::new(f32::MAX, f32::MIN), Vec2::new(f32::MAX, f32::MIN));
    for p in band(0.7, 0.85) {
        sx = Vec2::new(sx.x.min(p.x), sx.y.max(p.x));
        sz = Vec2::new(sz.x.min(p.z), sz.y.max(p.z));
    }
    let along_x = (sx.y - sx.x) >= (sz.y - sz.x);
    let mean = |it: &mut dyn Iterator<Item = &Vec3>| {
        let (mut s, mut n) = (Vec3::ZERO, 0.0);
        for p in it {
            s += *p;
            n += 1.0;
        }
        (n > 0.0).then(|| s / n)
    };
    let toes = mean(&mut band(0.0, 0.03));
    let ankles = mean(&mut band(0.06, 0.12));
    let fwd = match (toes, ankles) {
        (Some(t), Some(a)) => t - a,
        _ => Vec3::Z,
    };
    if along_x {
        // faces ±Z
        if fwd.z < -0.005 * h { 2 } else { 0 }
    } else if fwd.x > 0.0 {
        // faces +X: turn -90°
        -1
    } else {
        1
    }
}

/// The front view (x right, y up) of every triangle, as a grid of covered cells.
pub struct Silhouette {
    pub lo: Vec2,
    pub cell: f32,
    pub w: usize,
    pub h: usize,
    pub filled: Vec<bool>,
}

impl Silhouette {
    pub fn new(model: &Model, rows: usize) -> Option<Self> {
        Self::new_where(model, rows, |_| true)
    }
    /// Only the triangles whose middle passes `keep` (the frame is still the whole model's).
    pub fn new_where(model: &Model, rows: usize, keep: impl Fn(Vec3) -> bool) -> Option<Self> {
        let (lo, hi) = model.bounds()?;
        let height = (hi.y - lo.y).max(1e-4);
        let cell = height / rows as f32;
        let lo2 = Vec2::new(lo.x, lo.y) - Vec2::splat(cell);
        let w = ((hi.x - lo.x) / cell).ceil() as usize + 3;
        let h = rows + 3;
        let mut filled = vec![false; w * h];
        for m in &model.meshes {
            for t in m.indices.chunks_exact(3) {
                let mid = (m.positions[t[0] as usize]
                    + m.positions[t[1] as usize]
                    + m.positions[t[2] as usize])
                    / 3.0;
                if !keep(mid) {
                    continue;
                }
                let p = |i: u32| {
                    let v = m.positions[i as usize];
                    (Vec2::new(v.x, v.y) - lo2) / cell
                };
                fill_triangle(&mut filled, w, h, p(t[0]), p(t[1]), p(t[2]));
            }
        }
        Some(Self {
            lo: lo2,
            cell,
            w,
            h,
            filled,
        })
    }
    pub fn at(&self, x: f32, y: f32) -> bool {
        let c = ((Vec2::new(x, y) - self.lo) / self.cell).floor();
        if c.x < 0.0 || c.y < 0.0 {
            return false;
        }
        let (i, j) = (c.x as usize, c.y as usize);
        i < self.w && j < self.h && self.filled[j * self.w + i]
    }
    fn row_of(&self, y: f32) -> usize {
        (((y - self.lo.y) / self.cell).floor().max(0.0) as usize).min(self.h - 1)
    }
    /// The covered runs [x0, x1] (model units) of the row at height `y`.
    pub fn runs(&self, y: f32) -> Vec<(f32, f32)> {
        let j = self.row_of(y);
        let mut out = vec![];
        let mut start: Option<usize> = None;
        for i in 0..=self.w {
            let on = i < self.w && self.filled[j * self.w + i];
            match (on, start) {
                (true, None) => start = Some(i),
                (false, Some(s)) => {
                    out.push((
                        self.lo.x + s as f32 * self.cell,
                        self.lo.x + i as f32 * self.cell,
                    ));
                    start = None;
                }
                _ => {}
            }
        }
        out
    }
    /// The run at height `y` holding `x` (or the nearest one).
    pub fn run_at(&self, y: f32, x: f32) -> Option<(f32, f32)> {
        self.runs(y).into_iter().min_by(|a, b| {
            let d = |r: &(f32, f32)| {
                if x >= r.0 && x <= r.1 {
                    0.0
                } else {
                    (r.0 - x).abs().min((r.1 - x).abs())
                }
            };
            d(a).total_cmp(&d(b))
        })
    }
    /// The run at height `y` that covers `x`, if any.
    pub fn run_containing(&self, y: f32, x: f32) -> Option<(f32, f32)> {
        self.runs(y)
            .into_iter()
            .find(|(a, b)| x >= *a - self.cell && x <= *b + self.cell)
    }
    /// The covered runs [y0, y1] of the column at `x`.
    pub fn column_runs(&self, x: f32) -> Vec<(f32, f32)> {
        let i = (((x - self.lo.x) / self.cell).floor().max(0.0) as usize).min(self.w - 1);
        let mut out = vec![];
        let mut start: Option<usize> = None;
        for j in 0..=self.h {
            let on = j < self.h && self.filled[j * self.w + i];
            match (on, start) {
                (true, None) => start = Some(j),
                (false, Some(s)) => {
                    out.push((
                        self.lo.y + s as f32 * self.cell,
                        self.lo.y + j as f32 * self.cell,
                    ));
                    start = None;
                }
                _ => {}
            }
        }
        out
    }
    /// Follows an arm inward from `from` (a point on it) towards the body's middle `cx`, column
    /// by column, until it joins the torso (its covered column suddenly gets much taller).
    /// Returns the arm's centre line, outermost point first, and its typical thickness.
    pub fn trace_arm(&self, from: Vec2, cx: f32) -> Option<(Vec<Vec2>, f32)> {
        let dir = if from.x > cx { -1.0 } else { 1.0 };
        let mut y = from.y;
        // (a run's end is the edge of its cell: start inside it)
        let mut x = from.x + dir * self.cell * 0.5;
        let mut line = vec![];
        let mut thick: Vec<f32> = vec![];
        while (x - cx) * -dir > self.cell * 2.0 {
            let runs = self.column_runs(x);
            let Some((a, b)) = runs.into_iter().min_by(|p, q| {
                let d = |r: &(f32, f32)| {
                    if y >= r.0 && y <= r.1 { 0.0 } else { (r.0 - y).abs().min((r.1 - y).abs()) }
                };
                d(p).total_cmp(&d(q))
            }) else {
                break;
            };
            if y < a - 3.0 * self.cell || y > b + 3.0 * self.cell {
                break;
            }
            let t = b - a;
            // (the torso: far taller than any part of the arm so far, hand included)
            let widest = thick.iter().copied().fold(0.0f32, f32::max);
            if thick.len() >= 6 && t > widest * 2.2 {
                break;
            }
            thick.push(t);
            y = (a + b) * 0.5;
            line.push(Vec2::new(x, y));
            x += dir * self.cell;
        }
        if line.len() < 6 {
            return None;
        }
        let mut sorted = thick.clone();
        sorted.sort_by(f32::total_cmp);
        Some((line, sorted[sorted.len() / 2]))
    }
    pub fn torso_half_width(&self, groin_y: f32, chin_y: f32, cx: f32) -> f32 {
        let y = groin_y + (chin_y - groin_y) * 0.55;
        self.run_at(y, cx)
            .map(|(a, b)| (b - a) * 0.5)
            .unwrap_or((chin_y - groin_y) * 0.23)
    }
}

fn fill_triangle(filled: &mut [bool], w: usize, h: usize, a: Vec2, b: Vec2, c: Vec2) {
    let lo = a.min(b).min(c).floor().max(Vec2::ZERO);
    let hi = a.max(b).max(c).ceil();
    let (x1, y1) = ((hi.x as usize).min(w - 1), (hi.y as usize).min(h - 1));
    let area = (b - a).perp_dot(c - a);
    for j in lo.y as usize..=y1 {
        for i in lo.x as usize..=x1 {
            let p = Vec2::new(i as f32 + 0.5, j as f32 + 0.5);
            let inside = if area.abs() < 1e-9 {
                // a sliver: cover the cells it passes through
                (p - a).length() < 0.75 || (p - b).length() < 0.75 || (p - c).length() < 0.75
            } else {
                let w0 = (b - a).perp_dot(p - a) / area;
                let w1 = (c - b).perp_dot(p - b) / area;
                let w2 = (a - c).perp_dot(p - c) / area;
                // (half a cell of slack, so thin parts stay connected)
                let e = -0.5 / (area.abs().sqrt()).max(1.0);
                w0 >= e && w1 >= e && w2 >= e
            };
            if inside {
                filled[j * w + i] = true;
            }
        }
    }
}

/// The body's depth (z: back, front) around the front-view point (x, y).
pub fn depth_range(model: &Model, x: f32, y: f32, radius: f32) -> Option<(f32, f32)> {
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for m in &model.meshes {
        for p in &m.positions {
            if (p.x - x).abs() < radius && (p.y - y).abs() < radius {
                lo = lo.min(p.z);
                hi = hi.max(p.z);
            }
        }
    }
    (lo <= hi).then_some((lo, hi))
}

/// The middle of the body's depth (z) around the front-view point (x, y).
pub fn depth_mid(model: &Model, x: f32, y: f32, radius: f32) -> Option<f32> {
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for m in &model.meshes {
        for p in &m.positions {
            if (p.x - x).abs() < radius && (p.y - y).abs() < radius {
                lo = lo.min(p.z);
                hi = hi.max(p.z);
            }
        }
    }
    (lo <= hi).then(|| (lo + hi) * 0.5)
}

/// The shoulder joint of the arm through `elbow` (front view), traced along the arm to where it
/// meets the body; `None` if the arm can't be followed (the template's proportions then).
pub fn traced_shoulder(sil: &Silhouette, elbow: Vec2, cx: f32) -> Option<Vec2> {
    let (line, t) = sil.trace_arm(elbow, cx)?;
    Some(shoulder_of(&line, t, cx, false))
}

/// Where the shoulder joint sits, from the chin, groin and the torso's width.
pub fn shoulder(sil: &Silhouette, chin: Vec3, groin: Vec3, left: bool) -> Vec2 {
    let t = crate::humanoid::template();
    let s = (chin.y - groin.y) / (TEMPLATE.chin.y - TEMPLATE.groin.y);
    let cx = groin.x;
    let half = sil.torso_half_width(groin.y, chin.y, cx);
    let tsh = t.pos(if left { "upperarm_l" } else { "upperarm_r" });
    let x = cx + tsh.x * half / TEMPLATE_TORSO_HALF;
    let y = groin.y + (tsh.y - TEMPLATE.groin.y) * s;
    Vec2::new(x, y)
}

/// The eight markers, found on the model's front silhouette (stand it up with `normalize`
/// first). Never fails: anything it can't see falls back to the template's proportions.
pub fn guess(model: &Model) -> Markers {
    let Some((lo, hi)) = model.bounds() else {
        return TEMPLATE;
    };
    let h = hi.y - lo.y;
    let Some(sil) = Silhouette::new(model, 240) else {
        return TEMPLATE;
    };
    let cx = (lo.x + hi.x) * 0.5;
    let tpl_h = 1.8292;
    let rel = |v: f32| lo.y + v / tpl_h * h;
    // a first groin: going up the middle column from a quarter of the height, where the legs
    // join (the final one, below, also knows where the shoulders are)
    let groin0 = {
        let mut found = None;
        let step = sil.cell;
        let mut y = lo.y + 0.25 * h;
        while y < lo.y + 0.7 * h {
            let on = |y: f32| (-1..=1).any(|k| sil.at(cx + k as f32 * step, y));
            if on(y) && on(y + step) && on(y + 2.0 * step) && on(y + 3.0 * step) {
                found = Some(y);
                break;
            }
            y += step;
        }
        match found {
            Some(y) if y > lo.y + 0.3 * h => y,
            _ => rel(TEMPLATE.groin.y),
        }
    };
    // chin: coming down from the top, past the widest part of the head, the narrowest row
    // before the shoulders (or arms) widen it again; any head size
    let chin_y = neck_from_top(&sil, cx, lo.y, hi.y).unwrap_or_else(|| {
        let mut best: Option<(f32, f32)> = None;
        let mut y = lo.y + 0.76 * h;
        while y < lo.y + 0.95 * h {
            if let Some((a, b)) = sil.run_at(y, cx) {
                let wdt = b - a;
                if best.is_none_or(|(bw, _)| wdt < bw - 1e-6) {
                    best = Some((wdt, y));
                }
            }
            y += sil.cell;
        }
        best.map(|(_, y)| y).unwrap_or(rel(TEMPLATE.chin.y))
    });
    let s0 = (chin_y - groin0) / (TEMPLATE.chin.y - TEMPLATE.groin.y);
    let chin = Vec3::new(cx, chin_y, 0.0);
    // wrists and elbows: from the shoulder out to the furthest point of each arm
    let arm = |left: bool| -> (Vec3, Vec3, Vec2) {
        let mut sh = shoulder(&sil, chin, Vec3::new(cx, groin0, 0.0), left);
        let mut tip: Option<Vec2> = None;
        let mut y = lo.y + 0.2 * h;
        while y < chin_y + 0.06 * h {
            for (a, b) in sil.runs(y) {
                let x = if left { b } else { a };
                let further = tip.is_none_or(|t| if left { x > t.x } else { x < t.x });
                if further {
                    tip = Some(Vec2::new(x, y));
                }
            }
            y += sil.cell;
        }
        let tip = tip.unwrap_or(sh + Vec2::new(if left { 0.6 } else { -0.6 } * s0, 0.0));
        // follow the arm in from its tip: its own line and where it meets the body
        if let Some((line, t)) = sil.trace_arm(tip, cx) {
            sh = shoulder_of(&line, t, cx, true);
            // (the hand is the last ~30 % of the arm's length; the elbow halfway to the wrist)
            let wrist = along(&line, sh, WRIST_ALONG);
            let elbow = along(&line, sh, WRIST_ALONG * ELBOW_ALONG);
            return (wrist.extend(0.0), elbow.extend(0.0), sh);
        }
        let wrist = sh + (tip - sh) * WRIST_ALONG;
        let elbow = sh + (wrist - sh) * ELBOW_ALONG;
        (wrist.extend(0.0), elbow.extend(0.0), sh)
    };
    let (wrist_l, elbow_l, sh_l) = arm(true);
    let (wrist_r, elbow_r, sh_r) = arm(false);
    // the groin: going up the middle column from near the floor, the first place the legs join
    // and stay joined (feet touching don't count); it must sit a torso's length under the
    // shoulders (a tunic's or robe's front slit above the real one doesn't count either)
    let shoulder_y = (sh_l.y + sh_r.y) * 0.5;
    let groin_y = {
        let step = sil.cell;
        let on = |y: f32| (-1..=1).any(|k| sil.at(cx + k as f32 * step, y));
        let need = (0.05 * h / step).ceil() as usize;
        let (top, bottom) = (shoulder_y - 0.15 * h, shoulder_y - 0.5 * h);
        let mut found = None;
        let mut y = lo.y + 0.03 * h;
        let mut gap = false;
        while y < top {
            if !on(y) {
                gap = true;
            } else if gap && y > bottom && (1..need).all(|k| on(y + k as f32 * step)) {
                found = Some(y);
                break;
            }
            y += step;
        }
        found.unwrap_or_else(|| {
            if groin0 < top && groin0 > bottom {
                groin0
            } else {
                (shoulder_y - 0.33 * h).max(lo.y + 0.15 * h)
            }
        })
    };
    let s = (chin_y - groin_y) / (TEMPLATE.chin.y - TEMPLATE.groin.y);
    let chin = Vec3::new(cx, chin_y, 0.0);
    let groin = Vec3::new(cx, groin_y, 0.0);
    // knees: the middle of each leg at knee height
    let knee_y = groin_y * TEMPLATE.knee_l.y / TEMPLATE.groin.y;
    // the legs' middles: at knee height, or (under a skirt, a tunic or a robe) the highest row
    // below it where both legs show apart
    // (only what's near the body's depth: a tail or a cape hanging behind doesn't count)
    // (the body's depth at the groin: the solid's stretch there nearest the head's middle, so
    // a tail behind doesn't pull it back)
    let solid = crate::skin::Solid::new(model, SOLID_RES);
    let chin_z = depth_mid(model, cx, chin_y, 0.025 * h).unwrap_or(0.0);
    let groin_z = solid
        .as_ref()
        .and_then(|s| s.depth_near(cx, groin_y, chin_z))
        .map(|(a, b)| (a + b) * 0.5)
        .or_else(|| depth_mid(model, cx, groin_y, 0.025 * h))
        .unwrap_or(0.0);
    let legs = Silhouette::new_where(model, 240, |p| (p.z - groin_z).abs() < 0.12 * h);
    let legs = legs.as_ref().unwrap_or(&sil);
    let apart = |y: f32| {
        let r = legs.runs(y);
        let m = 0.02 * h;
        let l = r.iter().any(|(a, b)| (a + b) * 0.5 > cx + m);
        let rr = r.iter().any(|(a, b)| (a + b) * 0.5 < cx - m);
        (l && rr).then_some(r)
    };
    let runs = {
        let mut y = knee_y;
        loop {
            if let Some(r) = apart(y) {
                break r;
            }
            y -= legs.cell;
            if y < lo.y + 0.03 * h {
                break legs.runs(knee_y);
            }
        }
    };
    let leg = |left: bool| -> f32 {
        let fallback = cx + if left { 1.0 } else { -1.0 } * TEMPLATE.knee_l.x * s;
        runs.iter()
            .map(|(a, b)| (a + b) * 0.5)
            .filter(|m| if left { *m > cx } else { *m < cx })
            .min_by(|a, b| (a - fallback).abs().total_cmp(&(b - fallback).abs()))
            .unwrap_or(fallback)
    };
    let knee_l = Vec3::new(leg(true), knee_y, 0.0);
    let knee_r = Vec3::new(leg(false), knee_y, 0.0);
    let mut m = Markers {
        chin,
        groin,
        wrist_l,
        wrist_r,
        elbow_l,
        elbow_r,
        knee_l,
        knee_r,
    };
    // depths: the chin and groin first (the body's own), then the limbs in the body's layer
    // of the solid (a tail or cape behind doesn't pull them back)
    for k in Marker::ALL {
        let mut v = m.get(k);
        v.z = match (k, &solid) {
            (Marker::Chin, _) => Some(chin_z),
            (Marker::Groin, _) => Some(groin_z),
            (_, None) => depth_mid(model, v.x, v.y, 0.025 * h),
            (_, Some(s)) => s
                .depth_near(v.x, v.y, groin_z)
                .map(|(a, b)| (a + b) * 0.5)
                .or_else(|| depth_mid(model, v.x, v.y, 0.025 * h)),
        }
        .unwrap_or(0.0);
        m.set(k, v);
    }
    m
}

/// Voxels along the longest side of the solid the markers and the fit read depths from.
pub const SOLID_RES: usize = 160;

/// The neck: the narrowest row of the middle run that has a clearly wider head right above it
/// (within 8 % of the height) and clearly wider shoulders or arms right below (within 12 %).
/// That holds for any head size and skips hair tufts and ears on top. Rows where nothing covers
/// the middle count as width 0.
fn neck_from_top(sil: &Silhouette, cx: f32, lo: f32, hi: f32) -> Option<f32> {
    let h = hi - lo;
    let mut rows: Vec<(f32, f32)> = vec![];
    let mut y = hi - sil.cell * 0.5;
    while y > lo + 0.3 * h {
        let w = sil.run_containing(y, cx).map(|(a, b)| b - a).unwrap_or(0.0);
        rows.push((y, w));
        y -= sil.cell;
    }
    let above = (0.08 * h / sil.cell).ceil() as usize;
    let below = (0.12 * h / sil.cell).ceil() as usize;
    let mut best: Option<(f32, f32)> = None;
    for i in 0..rows.len() {
        let (y, w) = rows[i];
        if w <= 0.0 || y < lo + 0.4 * h {
            continue;
        }
        let head = rows[i.saturating_sub(above)..i].iter().map(|r| r.1).fold(0.0, f32::max);
        let shoulders = rows[i + 1..(i + 1 + below).min(rows.len())]
            .iter()
            .map(|r| r.1)
            .fold(0.0, f32::max);
        if head >= 1.3 * w && shoulders >= 1.5 * w && best.is_none_or(|(bw, _)| w < bw) {
            best = Some((w, y));
        }
    }
    best.map(|(_, y)| y)
}

/// Where an arm traced in from its tip (`trace_arm`) has its shoulder joint: where it met the
/// body, moved back out by part of the arm's thickness (the template's joint sits there), on the
/// upper arm's own line carried on into the body (an A-posed arm meets the body at the armpit,
/// well below the shoulder; right at the armpit the column also takes in some of the torso).
fn shoulder_of(line: &[Vec2], thickness: f32, cx: f32, from_tip: bool) -> Vec2 {
    let root = *line.last().unwrap();
    let tip = line[0];
    let out = if root.x > cx { 1.0 } else { -1.0 };
    // the upper arm: from a thickness out of the armpit to halfway to the tip (or all of a line
    // traced from the elbow)
    let span = (tip.x - root.x).abs() * if from_tip { 0.5 } else { 2.0 };
    let pts: Vec<Vec2> = line
        .iter()
        .copied()
        .filter(|p| {
            let d = (p.x - root.x).abs();
            d > thickness && d < span
        })
        .collect();
    if pts.len() < 4 {
        let x = root.x + out * thickness * SHOULDER_OUT;
        return Vec2::new(x, y_at(line, x + out * thickness));
    }
    // least squares y = a + b·x
    let n = pts.len() as f32;
    let (sx, sy) = pts.iter().fold((0.0, 0.0), |(a, b), p| (a + p.x, b + p.y));
    let (mx, my) = (sx / n, sy / n);
    let (mut sxx, mut sxy) = (0.0, 0.0);
    for p in &pts {
        sxx += (p.x - mx) * (p.x - mx);
        sxy += (p.x - mx) * (p.y - my);
    }
    let slope = if sxx > 1e-9 { sxy / sxx } else { 0.0 };
    // (a sloping arm's columns are taller than the arm is thick)
    let x = root.x + out * thickness / (1.0 + slope * slope).sqrt() * SHOULDER_OUT;
    Vec2::new(x, my + slope * (x - mx))
}

/// The traced line's height at `x` (nearest point).
fn y_at(line: &[Vec2], x: f32) -> f32 {
    line.iter()
        .min_by(|a, b| (a.x - x).abs().total_cmp(&(b.x - x).abs()))
        .map(|p| p.y)
        .unwrap_or(0.0)
}

/// The point `frac` of the way out from the shoulder `sh` to the arm's tip (`line[0]`): arms
/// in a T- or A-pose are nearly straight, so the fraction of the span is the fraction of the
/// length; its height is the arm's own there.
fn along(line: &[Vec2], sh: Vec2, frac: f32) -> Vec2 {
    let x = sh.x + (line[0].x - sh.x) * frac;
    Vec2::new(x, y_at(line, x))
}

/// The shoulder joint sits this many arm thicknesses out from where the arm meets the body.
pub const SHOULDER_OUT: f32 = 0.57;

/// Shoulder → wrist over shoulder → fingertip, and shoulder → elbow over shoulder → wrist.
pub const WRIST_ALONG: f32 = 0.70;
pub const ELBOW_ALONG: f32 = 0.50;

/// A marker the user moved on the front view keeps its depth: the fit puts every joint in the
/// middle of the body's depth itself (`crate::fit`).
pub fn with_depth(_model: &Model, v: Vec3) -> Vec3 {
    v
}
