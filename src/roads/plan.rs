//! Road plans: the centreline resampled every metre, the longitudinal profile (the ground
//! smoothed along the road so it has no bumps, cuts and fills limited per kind, bridge decks with
//! ramped approaches, ends pinned to the ground or to the road they join) and the bridge spans.
//! Plans are pure functions of the road, the ground before the roads and the water, so the bed
//! carving, the surface mesh, the bridges and the walk floors all agree.

use bevy::prelude::*;

use super::ground::Base;
use super::{AuthoredRoute, AuthoredSpanKind, Road, RoadKind, RoadOptions, Roads};
use crate::noise::smoothstep as ss;
use crate::water::WaterBodies;

/// Sample spacing along the centreline (m).
pub const DS: f32 = 1.0;
/// Longest water crossing that gets a bridge (m); longer crossings are left as fords.
pub const MAX_SPAN: f32 = 700.0;
/// Lowest deck clearance above water observed when an imported sheet is rebuilt after its water
/// changed. Authored heights otherwise remain exactly as supplied.
pub const AUTHORED_WATER_CLEARANCE: f32 = 0.25;
/// Water clearance blends into the authored profile over this much dry approach. This keeps a
/// water-level rebuild from introducing a step at a declared span boundary.
const AUTHORED_WATER_RAMP: f32 = 3.0;

#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub p: Vec2,
    /// Explicit profile height from the control polyline. Ordinary roads derive their final
    /// heights from terrain; authored routes retain this as their deck profile.
    pub y: f32,
    /// arc length from the start (m)
    pub s: f32,
    /// unit tangent (xz)
    pub dir: Vec2,
}

/// A bridge: the samples it covers (its abutments reach a little onto dry land).
#[derive(Clone, Debug)]
pub struct Span {
    pub i0: usize,
    pub i1: usize,
    /// water surface under it
    pub water: f32,
    /// deck height at the ends (stone decks arch a little higher in between)
    pub deck: f32,
    pub stone: bool,
    /// stone bridges: number of arches and their rise (m)
    pub arches: usize,
    pub rise: f32,
}

#[derive(Clone, Debug)]
pub struct RoadPlan {
    pub id: u32,
    pub kind: RoadKind,
    pub options: RoadOptions,
    /// The surface height is an explicit imported deck profile rather than terrain-derived.
    pub authored: bool,
    /// half width (m)
    pub hw: f32,
    pub samples: Vec<Sample>,
    /// ground before the roads under each sample
    pub ground: Vec<f32>,
    /// road surface height (the deck on bridges)
    pub h: Vec<f32>,
    /// bed strength: 1 on land, 0 under bridges and through fords
    pub bed: Vec<f32>,
    pub spans: Vec<Span>,
    /// how far the bed and paint reach from the centreline (m)
    pub reach: f32,
    pub lo: Vec2,
    pub hi: Vec2,
}

impl RoadPlan {
    pub fn length(&self) -> f32 {
        self.samples.last().map(|s| s.s).unwrap_or(0.0)
    }
    /// Sample index and fraction at arc length `s`.
    pub fn locate(&self, s: f32) -> (usize, f32) {
        let n = self.samples.len();
        if n < 2 {
            return (0, 0.0);
        }
        let step = self.length() / (n - 1) as f32;
        let x = (s / step.max(1e-6)).clamp(0.0, (n - 1) as f32);
        let i = (x.floor() as usize).min(n - 2);
        (i, x - i as f32)
    }
    fn lerp_at(&self, v: &[f32], s: f32) -> f32 {
        if v.len() < 2 {
            return v.first().copied().unwrap_or(0.0);
        }
        let (i, f) = self.locate(s);
        v[i] + (v[i + 1] - v[i]) * f
    }
    pub fn height_at(&self, s: f32) -> f32 {
        self.lerp_at(&self.h, s)
    }
    pub fn bed_at(&self, s: f32) -> f32 {
        self.lerp_at(&self.bed, s)
    }
    pub fn point_at(&self, s: f32) -> Vec2 {
        if self.samples.len() < 2 {
            return self.samples.first().map(|x| x.p).unwrap_or(Vec2::ZERO);
        }
        let (i, f) = self.locate(s);
        self.samples[i].p.lerp(self.samples[i + 1].p, f)
    }
    pub fn span_of(&self, i: usize) -> Option<&Span> {
        self.spans.iter().find(|sp| i >= sp.i0 && i <= sp.i1)
    }
    /// Visible, walkable sections of an imported deck. A sheet route can join distant plazas;
    /// only its declared bridge spans and short elevated banks belong to the bridge surface.
    /// Ranges use the planned ground snapshot, so they are stable for a visual rebuild and merge
    /// where nearby spans share a landing.
    pub fn authored_deck_ranges(&self) -> Vec<(usize, usize)> {
        if !self.authored || self.samples.len() < 2 {
            return vec![];
        }
        const BANK_MIN: f32 = 2.0;
        const BANK_MAX: f32 = 12.0;
        const GROUND_CLEARANCE: f32 = 0.12;
        let mut ranges = Vec::with_capacity(self.spans.len());
        for span in &self.spans {
            let (mut first, mut last) = (span.i0, span.i1);
            let s0 = self.samples[first].s;
            while first > 0 {
                let next = first - 1;
                let distance = s0 - self.samples[next].s;
                let above_ground = self.h[next] - self.ground[next] > GROUND_CLEARANCE;
                if distance <= BANK_MIN || (distance <= BANK_MAX && above_ground) {
                    first = next;
                } else {
                    break;
                }
            }
            let s1 = self.samples[last].s;
            while last + 1 < self.samples.len() {
                let next = last + 1;
                let distance = self.samples[next].s - s1;
                let above_ground = self.h[next] - self.ground[next] > GROUND_CLEARANCE;
                if distance <= BANK_MIN || (distance <= BANK_MAX && above_ground) {
                    last = next;
                } else {
                    break;
                }
            }
            ranges.push((first, last));
        }
        ranges.sort_unstable_by_key(|range| range.0);
        let mut merged: Vec<(usize, usize)> = vec![];
        for (first, last) in ranges {
            if let Some(previous) = merged.last_mut()
                && first <= previous.1.saturating_add(1)
            {
                previous.1 = previous.1.max(last);
            } else {
                merged.push((first, last));
            }
        }
        merged
    }
    /// Distance from `p` to the centreline and the arc length there (None beyond `max`).
    pub fn nearest(&self, p: Vec2, max: f32) -> Option<(f32, f32)> {
        if p.x < self.lo.x - max
            || p.y < self.lo.y - max
            || p.x > self.hi.x + max
            || p.y > self.hi.y + max
        {
            return None;
        }
        let mut best: Option<(f32, f32)> = None;
        for w in self.samples.windows(2) {
            let (a, b) = (w[0].p, w[1].p);
            let ab = b - a;
            let u = ((p - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
            let d = p.distance(a + ab * u);
            if d <= max && best.is_none_or(|x| d < x.0) {
                best = Some((d, w[0].s + (w[1].s - w[0].s) * u));
            }
        }
        best
    }
    /// Steepest and average grade (rise over run), measured over ~5 m.
    pub fn grades(&self) -> (f32, f32) {
        let n = self.h.len();
        let k = ((5.0 / DS).round() as usize).max(1);
        if n <= k {
            return (0.0, 0.0);
        }
        let (mut max, mut sum, mut cnt) = (0.0f32, 0.0f32, 0usize);
        let mut i = 0;
        while i + k < n {
            let run = (self.samples[i + k].s - self.samples[i].s).max(1e-3);
            let g = (self.h[i + k] - self.h[i]).abs() / run;
            max = max.max(g);
            sum += g;
            cnt += 1;
            i += k;
        }
        (max, sum / cnt.max(1) as f32)
    }
    /// Total climb and descent along the road (m).
    pub fn climb(&self) -> (f32, f32) {
        let (mut up, mut down) = (0.0, 0.0);
        for w in self.h.windows(2) {
            let d = w[1] - w[0];
            if d > 0.0 {
                up += d;
            } else {
                down -= d;
            }
        }
        (up, down)
    }
}

/// The centreline resampled at an even `ds` (xz arc length) with tangents.
pub fn dense_samples(road: &Road, ds: f32) -> Vec<Sample> {
    let line = road.centreline(ds * 0.25);
    if line.len() < 2 {
        return line
            .iter()
            .map(|p| Sample {
                p: p.xz(),
                y: p.y,
                s: 0.0,
                dir: Vec2::X,
            })
            .collect();
    }
    let mut cum = vec![0.0f32];
    for w in line.windows(2) {
        cum.push(cum.last().unwrap() + w[0].xz().distance(w[1].xz()));
    }
    let total = *cum.last().unwrap();
    let count = (total / ds).round().max(1.0) as usize;
    let step = total / count as f32;
    let mut out = Vec::with_capacity(count + 1);
    let mut seg = 0usize;
    for k in 0..=count {
        let s = k as f32 * step;
        while seg + 2 < cum.len() && cum[seg + 1] < s {
            seg += 1;
        }
        let (a, b) = (line[seg].xz(), line[seg + 1].xz());
        let len = cum[seg + 1] - cum[seg];
        let u = if len > 1e-6 {
            ((s - cum[seg]) / len).clamp(0.0, 1.0)
        } else {
            0.0
        };
        out.push(Sample {
            p: a.lerp(b, u),
            y: line[seg].y + (line[seg + 1].y - line[seg].y) * u,
            s,
            dir: Vec2::X,
        });
    }
    let n = out.len();
    for i in 0..n {
        let a = out[i.saturating_sub(1)].p;
        let b = out[(i + 1).min(n - 1)].p;
        out[i].dir = (b - a).normalize_or(Vec2::X);
    }
    out
}

/// Gaussian smoothing along the road: samples where `contrib` is false don't count, samples where
/// `modify` is false keep their value.
fn smooth(v: &[f32], contrib: &[bool], modify: &[bool], sigma: f32) -> Vec<f32> {
    let n = v.len() as isize;
    let r = (sigma * 3.0 / DS).ceil() as isize;
    let kern: Vec<f32> = (0..=r)
        .map(|k| (-0.5 * (k as f32 * DS / sigma).powi(2)).exp())
        .collect();
    (0..n)
        .map(|i| {
            if !modify[i as usize] {
                return v[i as usize];
            }
            let (mut sw, mut sv) = (0.0, 0.0);
            for k in -r..=r {
                let j = i + k;
                if j < 0 || j >= n || !contrib[j as usize] {
                    continue;
                }
                let w = kern[k.unsigned_abs()];
                sw += w;
                sv += w * v[j as usize];
            }
            if sw > 1e-6 { sv / sw } else { v[i as usize] }
        })
        .collect()
}

/// Water surface (lakes, rivers, the sea) over a point whose ground is `g`.
pub fn water_surface(water: &WaterBodies, sea: f32, p: Vec2, g: f32) -> f32 {
    water
        .water_level(Vec3::new(p.x, g, p.y))
        .unwrap_or(f32::NEG_INFINITY)
        .max(sea)
}

/// Plans one road. Where it ends on, or crosses, one of the already planned higher-ranked roads
/// in `joins`, its profile meets that road's height.
pub fn plan_road(road: &Road, base: &Base, water: &WaterBodies, joins: &[RoadPlan]) -> RoadPlan {
    let t = base.t;
    let st = road.kind.style();
    let hw = road.width * 0.5;
    let samples = dense_samples(road, DS);
    let n = samples.len();
    let ground: Vec<f32> = samples.iter().map(|s| base.at(s.p.x, s.p.y)).collect();
    let surface: Vec<f32> = samples
        .iter()
        .zip(&ground)
        .map(|(s, g)| water_surface(water, t.sea_level, s.p, *g))
        .collect();
    let wet: Vec<bool> = (0..n).map(|i| surface[i] - ground[i] > 0.2).collect();

    if let Some(authored) = road
        .authored
        .as_ref()
        .filter(|_| road.has_valid_authored_route())
    {
        return plan_authored(road, authored, hw, samples, ground, surface, t.cell);
    }

    // ---- bridge spans: wet runs (merged across small islands), abutments on dry land
    let mut spans: Vec<Span> = vec![];
    if road.options.bridges && n > 1 {
        let mut runs: Vec<(usize, usize)> = vec![];
        let mut i = 0;
        while i < n {
            if wet[i] {
                let a = i;
                while i + 1 < n && wet[i + 1] {
                    i += 1;
                }
                runs.push((a, i));
            }
            i += 1;
        }
        let gap = (8.0 / DS) as usize;
        let mut merged: Vec<(usize, usize)> = vec![];
        for r in runs {
            match merged.last_mut() {
                Some(m) if r.0 <= m.1 + gap => m.1 = r.1,
                _ => merged.push(r),
            }
        }
        let margin = ((1.5 + hw * 0.4).max(2.5) / DS).ceil() as usize;
        for (a, b) in merged {
            let wet_len = samples[b].s - samples[a].s + DS;
            if wet_len > MAX_SPAN {
                continue;
            }
            let (i0, i1) = (a.saturating_sub(margin), (b + margin).min(n - 1));
            // a span merging into the previous one (two channels close together)
            if let Some(prev) = spans.last_mut()
                && i0 <= prev.i1 + 1
            {
                prev.i1 = i1;
                continue;
            }
            let water_lv = (a..=b).map(|k| surface[k]).fold(f32::MIN, f32::max);
            let len = samples[i1].s - samples[i0].s;
            let stone = st.stone_bridge;
            let (arches, rise) = if stone {
                super::bridge::stone_layout(len, wet_len)
            } else {
                (0, 0.0)
            };
            let clear = if stone {
                0.5 + rise + 0.9
            } else {
                (1.1 + 0.015 * wet_len).min(2.5)
            };
            let deck = (water_lv + clear)
                .max(ground[i0] + 0.3)
                .max(ground[i1] + 0.3);
            spans.push(Span {
                i0,
                i1,
                water: water_lv,
                deck,
                stone,
                arches,
                rise,
            });
        }
    }
    let mut in_span = vec![false; n];
    for sp in &spans {
        for f in &mut in_span[sp.i0..=sp.i1] {
            *f = true;
        }
    }

    // ---- profile: smoothed ground, limited cut / fill, smoothed again
    let mut contrib: Vec<bool> = (0..n).map(|i| !wet[i] && !in_span[i]).collect();
    if !contrib.iter().any(|c| *c) {
        contrib = vec![true; n];
    }
    let modify: Vec<bool> = in_span.iter().map(|s| !s).collect();
    let mut h = smooth(&ground, &contrib, &modify, st.smooth);
    for i in 0..n {
        if contrib[i] {
            h[i] = h[i].clamp(ground[i] - st.cut, ground[i] + st.cut);
        }
    }
    let mut h = smooth(&h, &contrib, &modify, st.smooth * 0.5);

    // ---- bridge decks and their approach ramps; a deck is never below the road leading onto it
    // (a crossing in a gorge stays level with its rims)
    for sp in spans.iter_mut() {
        let before = if sp.i0 > 0 { h[sp.i0 - 1] } else { sp.deck };
        let after = if sp.i1 + 1 < n { h[sp.i1 + 1] } else { sp.deck };
        sp.deck = sp.deck.max(before.min(after));
    }
    let approach = if st.stone_bridge {
        0.07
    } else {
        st.max_grade.min(0.1)
    };
    for sp in &spans {
        let (s0, s1) = (samples[sp.i0].s, samples[sp.i1].s);
        let len = (s1 - s0).max(1.0);
        let hump = if sp.stone {
            (0.035 * len).min(1.6)
        } else {
            (0.01 * len).min(0.35)
        };
        for i in sp.i0..=sp.i1 {
            let u = (samples[i].s - s0) / len;
            h[i] = sp.deck + hump * 4.0 * u * (1.0 - u);
        }
        // an embankment up to the deck or a cutting down to it, out to where the road meets it
        let ramp = |h: &mut [f32], i: usize, d: f32| -> bool {
            let v = h[i].clamp(sp.deck - approach * d, sp.deck + approach * d);
            let hit = v != h[i];
            h[i] = v;
            hit
        };
        for i in (0..sp.i0).rev() {
            if in_span[i] || !ramp(&mut h, i, s0 - samples[i].s) {
                break;
            }
        }
        for i in sp.i1 + 1..n {
            if in_span[i] || !ramp(&mut h, i, samples[i].s - s1) {
                break;
            }
        }
    }
    // rounded vertical curves where ramps meet the ground
    let all = vec![true; n];
    let mut h = if spans.is_empty() {
        h
    } else {
        smooth(&h, &all, &modify, 3.0)
    };

    // ---- junctions: the ends sit on the ground or on the road they join; crossings meet the
    // crossed road's height
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for s in &samples {
        lo = lo.min(s.p);
        hi = hi.max(s.p);
    }
    if n > 1 {
        let len = samples[n - 1].s;
        let on_road = |p: Vec2| {
            joins
                .iter()
                .find_map(|o| o.nearest(p, o.hw + 1.0).map(|(_, s)| o.height_at(s)))
        };
        let mut pins: Vec<(usize, f32)> = vec![
            (0, on_road(samples[0].p).unwrap_or(ground[0])),
            (n - 1, on_road(samples[n - 1].p).unwrap_or(ground[n - 1])),
        ];
        for o in joins {
            if o.hi.x < lo.x || o.hi.y < lo.y || o.lo.x > hi.x || o.lo.y > hi.y {
                continue;
            }
            let mut best: Option<(f32, usize, f32)> = None;
            for (i, sm) in samples.iter().enumerate().step_by(2) {
                let inside = sm.s > hw + 3.0 && sm.s < len - hw - 3.0;
                match o.nearest(sm.p, 1.5).filter(|_| inside) {
                    Some((d, so)) => {
                        if best.is_none_or(|b| d < b.0) {
                            best = Some((d, i, o.height_at(so)));
                        }
                    }
                    None => {
                        if let Some(b) = best.take() {
                            pins.push((b.1, b.2));
                        }
                    }
                }
            }
            if let Some(b) = best {
                pins.push((b.1, b.2));
            }
        }
        for (e, target) in pins {
            if in_span[e] {
                continue;
            }
            let delta = target - h[e];
            // spread the correction so it adds at most ~60% of the design grade
            let l = (st.smooth * 2.0)
                .max(6.0)
                .max(delta.abs() / (st.max_grade * 0.6))
                .min(len * 0.5)
                .max(1.0);
            let se = samples[e].s;
            for i in 0..n {
                let d = (samples[i].s - se).abs();
                if d < l && !in_span[i] {
                    h[i] += delta * (1.0 - ss(0.0, l, d));
                }
            }
        }
    }

    let bed: Vec<f32> = (0..n)
        .map(|i| if in_span[i] || wet[i] { 0.0 } else { 1.0 })
        .collect();
    let fill = (0..n)
        .filter(|i| bed[*i] > 0.0)
        .map(|i| (h[i] - ground[i]).abs())
        .fold(0.0f32, f32::max);
    let core = hw + st.edge * 0.5 + 0.35 * t.cell;
    let bed_reach = core + (fill * 1.6).clamp((t.cell * 1.25).max(2.5), 16.0) + 0.5;
    let paint_reach = (hw + st.under_reach).max(0.35 * t.cell) + st.shoulder_w * 1.8 + 1.8;
    let reach = bed_reach.max(paint_reach);
    RoadPlan {
        id: road.id,
        kind: road.kind,
        options: road.options,
        authored: false,
        hw,
        samples,
        ground,
        h,
        bed,
        spans,
        reach,
        lo: lo - Vec2::splat(reach),
        hi: hi + Vec2::splat(reach),
    }
}

/// Plans a sheet-authored deck without smoothing it against terrain. The bridge structure still
/// needs spans, but the road ribbon and walk floor use the complete explicit profile, including
/// raised dry approaches.
fn plan_authored(
    road: &Road,
    authored: &AuthoredRoute,
    hw: f32,
    samples: Vec<Sample>,
    ground: Vec<f32>,
    surface: Vec<f32>,
    cell: f32,
) -> RoadPlan {
    let n = samples.len();
    debug_assert!(n >= 2);
    let len = samples.last().map_or(0.0, |s| s.s).max(1e-6);
    let mut h: Vec<f32> = samples.iter().map(|s| s.y).collect();
    // Valid authored points are finite, but retain a safe fallback if this function is reused.
    for (i, v) in h.iter_mut().enumerate() {
        if !v.is_finite() {
            *v = ground[i];
        }
    }

    let mut source = authored
        .spans
        .iter()
        .filter_map(|span| {
            if !span.start.is_finite() || !span.end.is_finite() || !span.support_y.is_finite() {
                return None;
            }
            let (start, end) = (span.start.clamp(0.0, 1.0), span.end.clamp(0.0, 1.0));
            (end - start > 1e-4).then_some((start, end, span))
        })
        .collect::<Vec<_>>();
    source.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));

    let mut pending = Vec::with_capacity(source.len());
    let mut last_i1 = None;
    for (start, end, source) in source {
        let s0 = start * len;
        let s1 = end * len;
        // Expand out to surrounding samples so the declared hazard never peeks out from under
        // a structural deck because a normalized boundary fell between metre samples.
        let i0 = samples
            .partition_point(|sample| sample.s <= s0)
            .saturating_sub(1)
            .min(n - 1);
        let i1 = samples.partition_point(|sample| sample.s < s1).min(n - 1);
        if i1 <= i0 || last_i1.is_some_and(|last| i0 <= last) {
            continue;
        }
        let observed_water = match source.kind {
            AuthoredSpanKind::Water => surface[i0..=i1]
                .iter()
                .copied()
                .filter(|v| v.is_finite())
                .fold(f32::NEG_INFINITY, f32::max),
            AuthoredSpanKind::Chasm => f32::NEG_INFINITY,
        };
        pending.push((i0, i1, source.support_y.max(observed_water), source.kind));
        last_i1 = Some(i1);
    }

    // Raise only declared water crossings. Apply the same lift across the whole span, then fade
    // it into the approaches, so the source profile stays smooth if a lake or river rises.
    let mut lift = vec![0.0f32; n];
    for &(i0, i1, water_y, kind) in &pending {
        if kind != AuthoredSpanKind::Water || !water_y.is_finite() {
            continue;
        }
        let required = water_y + AUTHORED_WATER_CLEARANCE;
        let lowest = h[i0..=i1].iter().copied().fold(f32::INFINITY, f32::min);
        let amount = (required - lowest).max(0.0);
        if amount <= 0.0 {
            continue;
        }
        let s0 = samples[i0].s;
        let s1 = samples[i1].s;
        for (i, sample) in samples.iter().enumerate() {
            let outside = if sample.s < s0 {
                s0 - sample.s
            } else if sample.s > s1 {
                sample.s - s1
            } else {
                0.0
            };
            let blend = (1.0 - outside / AUTHORED_WATER_RAMP).clamp(0.0, 1.0);
            lift[i] = lift[i].max(amount * blend);
        }
    }
    for (height, amount) in h.iter_mut().zip(lift) {
        *height += amount;
    }

    let mut spans = Vec::with_capacity(pending.len());
    for (i0, i1, water, _) in pending {
        let span_len = samples[i1].s - samples[i0].s;
        let wet_len = span_len.max(DS);
        let stone = road.kind.style().stone_bridge;
        let (arches, rise) = if stone {
            super::bridge::stone_layout(span_len, wet_len)
        } else {
            (0, 0.0)
        };
        spans.push(Span {
            i0,
            i1,
            water,
            deck: h[i0..=i1].iter().copied().fold(f32::NEG_INFINITY, f32::max),
            stone,
            arches,
            rise,
        });
    }

    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for sample in &samples {
        lo = lo.min(sample.p);
        hi = hi.max(sample.p);
    }
    let st = road.kind.style();
    let reach = (hw + st.under_reach).max(0.35 * cell) + st.shoulder_w * 1.8 + 1.8;
    RoadPlan {
        id: road.id,
        kind: road.kind,
        options: road.options,
        authored: true,
        hw,
        samples,
        ground,
        h,
        // Authored decks are additive. Their y values are intentionally not fed back into the
        // terrain, including dry elevated approaches.
        bed: vec![0.0; n],
        spans,
        reach,
        lo: lo - Vec2::splat(reach),
        hi: hi + Vec2::splat(reach),
    }
}

/// Plans every road, sorted by rank (lowest first, the carving order). Higher-ranked roads are
/// planned first so a road joining or crossing another one takes its height there.
pub fn plan_all(roads: &Roads, base: &Base, water: &WaterBodies) -> Vec<RoadPlan> {
    let mut order: Vec<&Road> = roads
        .roads
        .iter()
        .filter(|r| {
            r.points.len() >= 2
                && r.points
                    .iter()
                    .all(|p| p.iter().copied().all(f32::is_finite))
        })
        .collect();
    order.sort_by_key(|r| std::cmp::Reverse(r.rank()));
    let mut plans: Vec<RoadPlan> = Vec::with_capacity(order.len());
    for r in order {
        let plan = plan_road(r, base, water, &plans);
        plans.push(plan);
    }
    plans.reverse();
    plans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roads::{AuthoredRoute, AuthoredSpan, AuthoredSpanKind, RoadOptions};
    use crate::terrain::Terrain;

    fn terrain() -> Terrain {
        let mut terrain = Terrain::new(20, 1.0, -100.0);
        for j in 0..=terrain.res {
            for i in 0..=terrain.res {
                let height = (i as f32 - 10.0) * 0.7 + (j as f32 - 10.0) * 0.2;
                let index = terrain.idx(i, j);
                terrain.heights[index] = height;
            }
        }
        terrain
    }

    fn authored(points: Vec<[f32; 3]>, spans: Vec<AuthoredSpan>) -> Road {
        let mut road = Road::new(
            41,
            RoadKind::DirtRoad,
            points,
            RoadKind::DirtRoad.default_width(),
            RoadOptions::default(),
        );
        road.authored = Some(AuthoredRoute { spans });
        road
    }

    fn span(kind: AuthoredSpanKind, start: f32, end: f32, support_y: f32) -> AuthoredSpan {
        AuthoredSpan {
            start,
            end,
            support_y,
            kind,
        }
    }

    #[test]
    fn authored_dry_chasm_keeps_explicit_deck_and_never_carves_terrain() {
        let terrain = terrain();
        let ground = crate::roads::ground::GroundStore::default();
        let base = Base {
            t: &terrain,
            g: &ground,
        };
        let road = authored(
            vec![[-8.0, 4.0, 0.0], [0.0, 9.0, 0.0], [8.0, 6.0, 0.0]],
            vec![span(AuthoredSpanKind::Chasm, 0.2, 0.8, -30.0)],
        );
        let plan = plan_road(&road, &base, &WaterBodies::default(), &[]);

        assert!(plan.authored);
        assert_eq!(plan.spans.len(), 1);
        assert!((plan.height_at(0.0) - 4.0).abs() < 1e-4);
        assert!((plan.height_at(8.0) - 9.0).abs() < 1e-4);
        assert!((plan.height_at(plan.length()) - 6.0).abs() < 1e-4);
        assert!(plan.bed.iter().all(|strength| *strength == 0.0));
        let floors = crate::roads::render::floor_decks(&plan);
        assert!(
            floors.len() >= 3,
            "the deck has dry approaches on both sides of the chasm"
        );
        let road_floors = crate::roads::render::RoadFloors {
            decks: floors,
            roads: vec![],
        };
        for (x, expected_y) in [(-6.0, 5.25), (0.0, 9.0), (6.0, 6.75)] {
            let floor = road_floors.floor_at(Vec3::new(x, expected_y + 0.25, 0.0));
            assert!(
                floor.is_some_and(|(y, _)| (y - expected_y).abs() < 1e-4),
                "missing continuous authored floor at x={x}"
            );
        }
    }

    #[test]
    fn authored_uneven_endpoints_stay_linear_in_xz_not_spline_smoothed() {
        let terrain = terrain();
        let ground = crate::roads::ground::GroundStore::default();
        let base = Base {
            t: &terrain,
            g: &ground,
        };
        let road = authored(
            vec![[-8.0, 2.0, -2.0], [-2.0, 11.0, -2.0], [8.0, 5.0, -2.0]],
            vec![],
        );
        let plan = plan_road(&road, &base, &WaterBodies::default(), &[]);

        assert!((plan.height_at(3.0) - 6.5).abs() < 1e-4);
        assert!((plan.height_at(6.0) - 11.0).abs() < 1e-4);
        assert!((plan.height_at(11.0) - 8.0).abs() < 1e-4);
    }

    #[test]
    fn authored_ranges_exclude_a_long_dry_approach() {
        let samples: Vec<Sample> = (0..=40)
            .map(|i| Sample {
                p: Vec2::new(i as f32, 0.0),
                y: if (18..=22).contains(&i) { 3.0 } else { 0.0 },
                s: i as f32,
                dir: Vec2::X,
            })
            .collect();
        let plan = RoadPlan {
            id: 9,
            kind: RoadKind::Footpath,
            options: RoadOptions::default(),
            authored: true,
            hw: 1.0,
            ground: vec![0.0; samples.len()],
            h: samples.iter().map(|sample| sample.y).collect(),
            bed: vec![0.0; samples.len()],
            spans: vec![Span {
                i0: 18,
                i1: 22,
                water: -2.0,
                deck: 3.0,
                stone: false,
                arches: 0,
                rise: 0.0,
            }],
            reach: 2.0,
            lo: Vec2::ZERO,
            hi: Vec2::new(40.0, 0.0),
            samples,
        };
        assert_eq!(plan.authored_deck_ranges(), vec![(16, 24)]);
    }

    #[test]
    fn authored_water_span_clamps_to_current_water_clearance() {
        let terrain = terrain();
        let ground = crate::roads::ground::GroundStore::default();
        let base = Base {
            t: &terrain,
            g: &ground,
        };
        let water = WaterBodies {
            level_map: vec![7.0; terrain.n() * terrain.n()],
            level_res: terrain.res,
            grid_cache: Some((terrain.half(), terrain.cell)),
            ..default()
        };
        let road = authored(
            vec![[-8.0, 3.0, 1.0], [8.0, 3.0, 1.0]],
            vec![span(AuthoredSpanKind::Water, 0.25, 0.75, 4.0)],
        );
        let plan = plan_road(&road, &base, &water, &[]);

        assert_eq!(plan.spans.len(), 1);
        let span = &plan.spans[0];
        assert!(
            plan.h[span.i0..=span.i1]
                .iter()
                .all(|deck_y| *deck_y >= 7.0 + AUTHORED_WATER_CLEARANCE)
        );
        assert!((plan.h[0] - 3.0).abs() < 1e-4);
        assert!((plan.h.last().unwrap() - 3.0).abs() < 1e-4);
        assert!((plan.spans[0].water - 7.0).abs() < 1e-4);
    }

    #[test]
    fn ordinary_roads_keep_ground_derived_profile_despite_control_point_y() {
        let terrain = terrain();
        let ground = crate::roads::ground::GroundStore::default();
        let base = Base {
            t: &terrain,
            g: &ground,
        };
        let road = Road::new(
            42,
            RoadKind::DirtRoad,
            vec![[-8.0, 300.0, 3.0], [8.0, -300.0, 3.0]],
            RoadKind::DirtRoad.default_width(),
            RoadOptions::default(),
        );
        let plan = plan_road(&road, &base, &WaterBodies::default(), &[]);

        assert!(!plan.authored);
        assert!(plan.spans.is_empty());
        assert!((plan.h[0] - plan.ground[0]).abs() < 1e-4);
        assert!((plan.h.last().unwrap() - plan.ground.last().unwrap()).abs() < 1e-4);
        assert!(plan.bed.iter().any(|strength| *strength > 0.0));
    }
}
