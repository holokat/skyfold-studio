//! Cloth: rectangles of particles held together by stretch, bending and
//! tether constraints, pushed by wind (port of `cloth/`).
//!
//! Per substep: multipliers are reset and the aerodynamic drag and lift
//! (Keckeisen et al. 2004) move the predicted positions; every solver
//! iteration runs the distance, bending and tether constraint groups; after
//! the velocity update, an optional damping pass blends each particle's
//! velocity toward its edge neighbors'. Extensions: pinned points can be moved
//! by the host (interpolated across substeps), and each particle tracks a
//! wetness that rises when liquid touches it and dries over time.

pub mod constraints;
pub mod graph;
pub mod surface;
pub mod tether;

use crate::par::SharedMut;
use crate::particles::Particles;
use constraints::ConstraintSet;
use glam::{Quat, Vec2, Vec3, Vec3A};
use graph::ClothGraph;
use rayon::prelude::*;

/// Which particles of a rectangular cloth are held in place.
#[derive(Clone, Debug, PartialEq)]
pub enum ClothPins {
    /// The whole top row.
    Top,
    /// The two top corners.
    TopCorners,
    /// All four corners.
    Corners,
    /// Nothing.
    None,
    /// The grid vertices nearest these `(u, v)` points; `u` runs 0→1 left to
    /// right across the width, `v` 0→1 top to bottom.
    Uv(Vec<Vec2>),
    /// Every grid vertex along these `(u, v)` segments, e.g. a tent ridge
    /// `[(0, 0.5), (1, 0.5)]`.
    UvLines(Vec<[Vec2; 2]>),
    /// Grid vertex indices (`row · (columns + 1) + column`, row 0 at the top).
    /// The grid size depends on the particle radius; see [`ClothOptions::grid_size`].
    Indices(Vec<u32>),
}

/// A rectangle of cloth. Defaults follow the reference `Simulation.addCloth`.
#[derive(Clone, Debug, PartialEq)]
pub struct ClothOptions {
    /// Width and height, m.
    pub width: f32,
    pub height: f32,
    /// Centre of the cloth. Default `(0, 1, 0)`.
    pub position: Vec3,
    /// Orientation; the cloth starts upright in the XY plane facing +z.
    /// `Quat::from_rotation_x(-FRAC_PI_2)` lays it flat (front facing up).
    pub rotation: Quat,
    pub pins: ClothPins,
    /// 0 for stiff canvas to 1 for silk. Default 0.75.
    pub softness: f32,
    /// Mass per area, kg/m². Default 0.1. Heavier cloth holds liquid with less leaking.
    pub weight: f32,
    /// Wind velocity, m/s. Change it every frame with `set_cloth_wind`.
    pub wind: Vec3,
    /// Particle spacing in particle radii. Default 2.2: above 2, so neighbors
    /// never touch and the cloth can fold onto itself.
    pub spacing: f32,
    /// Let the cloth's particles collide with each other. Default true. Turn
    /// off when `spacing < 2`, where neighbors would always be in contact.
    pub self_collision: bool,
    /// Stretch compliance, s²/kg. Default 1e-7.
    pub stretch_compliance: f32,
    /// Tether compliance, s²/kg. Default 0 (rigid).
    pub tether_compliance: f32,
    /// Slack in each tether as a fraction of its rest length. Default 0.06.
    pub stretch_tolerance: f32,
    /// Air drag `½ C_D ρ_air`, kg/m³. Default 0.18.
    pub drag: f32,
    /// Air lift `½ C_L ρ_air`, kg/m³. Default 0.02.
    pub lift: f32,
    /// Velocity smoothing toward edge neighbors, 0–1. Default `Some(0.1)`.
    pub damping: Option<f32>,
    /// Explicit bending compliance, overriding the `softness` formula.
    pub bend_compliance: Option<f32>,
    /// How fast liquid wets the cloth: per second per touching liquid particle,
    /// as an exponential approach to 1. Default 3.
    pub wet_rate: f32,
    /// How fast the cloth dries, wetness per second. Default 1/90 (a soaked
    /// cloth dries in a minute and a half).
    pub dry_rate: f32,
}

impl Default for ClothOptions {
    fn default() -> Self {
        Self {
            width: 1.0,
            height: 1.0,
            position: Vec3::new(0.0, 1.0, 0.0),
            rotation: Quat::IDENTITY,
            pins: ClothPins::Top,
            softness: 0.75,
            weight: 0.1,
            wind: Vec3::ZERO,
            spacing: 2.2,
            self_collision: true,
            stretch_compliance: 1e-7,
            tether_compliance: 0.0,
            stretch_tolerance: 0.06,
            drag: 0.18,
            lift: 0.02,
            damping: Some(0.1),
            bend_compliance: None,
            wet_rate: 3.0,
            dry_rate: 1.0 / 90.0,
        }
    }
}

impl ClothOptions {
    /// Grid cells `(columns, rows)` this cloth gets at `particle_radius`
    /// (particles per row are `columns + 1`).
    pub fn grid_size(&self, particle_radius: f32) -> (u32, u32) {
        let s = self.spacing * particle_radius;
        (
            (self.width / s).round().max(2.0) as u32,
            (self.height / s).round().max(2.0) as u32,
        )
    }

    pub(crate) fn pinned_indices(&self, columns: u32, rows: u32) -> Vec<u32> {
        let width = columns + 1;
        let corner = |c: u32, r: u32| r * width + c;
        let nearest = |uv: Vec2| {
            let c = (uv.x.clamp(0.0, 1.0) * columns as f32).round() as u32;
            let r = (uv.y.clamp(0.0, 1.0) * rows as f32).round() as u32;
            corner(c, r)
        };
        let mut out: Vec<u32> = match &self.pins {
            ClothPins::Top => (0..width).collect(),
            ClothPins::TopCorners => vec![corner(0, 0), corner(columns, 0)],
            ClothPins::Corners => vec![
                corner(0, 0),
                corner(columns, 0),
                corner(0, rows),
                corner(columns, rows),
            ],
            ClothPins::None => Vec::new(),
            ClothPins::Uv(points) => points.iter().map(|&p| nearest(p)).collect(),
            ClothPins::UvLines(lines) => {
                let steps = 4 * columns.max(rows);
                lines
                    .iter()
                    .flat_map(|[a, b]| {
                        (0..=steps).map(move |k| a.lerp(*b, k as f32 / steps as f32))
                    })
                    .map(nearest)
                    .collect()
            }
            ClothPins::Indices(list) => list.clone(),
        };
        out.sort_unstable();
        out.dedup();
        out.retain(|&i| i < width * (rows + 1));
        out
    }
}

/// The reference's bend compliance formula: the same drape at any resolution.
pub fn cloth_bend_compliance(softness: f32, segments: u32, weight: f32) -> f32 {
    10f32.powf(-1.0 + softness * 5.0) * (segments as f32 / 30.0).powi(4) * (0.35 / weight)
}

/// A built cloth.
pub(crate) struct Cloth {
    pub start: u32,
    pub count: u32,
    /// Particles per row and number of rows.
    pub columns: u32,
    pub rows: u32,
    pub graph: ClothGraph,
    pub distance: ConstraintSet<2>,
    pub bending: Option<ConstraintSet<4>>,
    pub tethers: Option<(ConstraintSet<1>, Vec<u32>)>,
    tri_offsets: Vec<u32>,
    tri_list: Vec<u32>,
    nbr_offsets: Vec<u32>,
    nbr_list: Vec<u32>,
    pub wind: Vec3A,
    pub drag: f32,
    pub lift: f32,
    pub damping: Option<f32>,
    pub softness: f32,
    pub weight: f32,
    pub explicit_bend: Option<f32>,
    /// Pinned vertices (cloth-local), their rest positions, and host targets.
    pub pins: Vec<u32>,
    pub pin_rest: Vec<Vec3>,
    pub rest_center: Vec3,
    pin_target: Vec<Option<Vec3A>>,
    pin_frame_start: Vec<Vec3A>,
    pub wetness: Vec<f32>,
    pub wet_rate: f32,
    pub dry_rate: f32,
    smoothed: Vec<Vec3A>,
    tri_force: Vec<Vec3A>,
}

impl Cloth {
    pub fn new(
        start: u32,
        columns: u32,
        rows: u32,
        graph: ClothGraph,
        opts: &ClothOptions,
    ) -> Self {
        let count = graph.positions.len() as u32;
        let g = |i: u32| start + i;
        let distance = ConstraintSet::new(
            graph
                .distance_pairs
                .iter()
                .map(|&[a, b]| [g(a), g(b)])
                .collect(),
            graph.distance_rest_lengths.clone(),
            opts.stretch_compliance,
        );
        let bend_compliance = opts
            .bend_compliance
            .unwrap_or_else(|| cloth_bend_compliance(opts.softness, columns - 1, opts.weight));
        let bending = (!graph.bending_tuples.is_empty()).then(|| {
            ConstraintSet::new(
                graph.bending_tuples.iter().map(|t| t.map(g)).collect(),
                graph.bending_rest_angles.clone(),
                bend_compliance,
            )
        });
        let tether_list = tether::build_tethers(&graph, opts.stretch_tolerance, 4);
        let tethers = (!tether_list.is_empty()).then(|| {
            (
                ConstraintSet::new(
                    tether_list.iter().map(|t| [g(t.particle)]).collect(),
                    tether_list.iter().map(|t| t.rest_radius).collect(),
                    opts.tether_compliance,
                ),
                tether_list.iter().map(|t| g(t.anchor)).collect(),
            )
        });
        let n = count as usize;
        let csr = |lists: Vec<Vec<u32>>| {
            let mut offsets = vec![0u32; n + 1];
            for (i, l) in lists.iter().enumerate() {
                offsets[i + 1] = offsets[i] + l.len() as u32;
            }
            (offsets, lists.into_iter().flatten().collect::<Vec<u32>>())
        };
        let mut incident = vec![Vec::new(); n];
        for (t, tri) in graph.triangles.iter().enumerate() {
            for &v in tri {
                incident[v as usize].push(t as u32);
            }
        }
        let (tri_offsets, tri_list) = csr(incident);
        let mut nbrs = vec![Vec::new(); n];
        for &[a, b] in &graph.distance_pairs {
            nbrs[a as usize].push(b);
            nbrs[b as usize].push(a);
        }
        let (nbr_offsets, nbr_list) = csr(nbrs);
        let pins: Vec<u32> = (0..count)
            .filter(|&i| graph.inv_mass[i as usize] == 0.0)
            .collect();
        let pin_rest: Vec<Vec3> = pins.iter().map(|&i| graph.positions[i as usize]).collect();
        Self {
            start,
            count,
            columns,
            rows,
            distance,
            bending,
            tethers,
            tri_offsets,
            tri_list,
            nbr_offsets,
            nbr_list,
            wind: opts.wind.into(),
            drag: opts.drag,
            lift: opts.lift,
            damping: opts.damping,
            softness: opts.softness,
            weight: opts.weight,
            explicit_bend: opts.bend_compliance,
            pin_target: vec![None; pins.len()],
            pin_frame_start: pin_rest.iter().map(|&p| p.into()).collect(),
            pins,
            pin_rest,
            rest_center: opts.position,
            wetness: vec![0.0; n],
            wet_rate: opts.wet_rate,
            dry_rate: opts.dry_rate,
            smoothed: vec![Vec3A::ZERO; n],
            tri_force: Vec::new(),
            graph,
        }
    }

    pub fn range(&self) -> std::ops::Range<usize> {
        self.start as usize..(self.start + self.count) as usize
    }

    /// Recompute the bending compliance after `softness` changed.
    pub fn set_softness(&mut self, softness: f32) {
        self.softness = softness;
        if let Some(b) = &mut self.bending {
            b.compliance = self
                .explicit_bend
                .unwrap_or_else(|| cloth_bend_compliance(softness, self.columns - 1, self.weight));
        }
    }

    pub fn set_pin_target(&mut self, pin: usize, position: Vec3) {
        if let Some(t) = self.pin_target.get_mut(pin) {
            *t = Some(position.into());
        }
    }

    /// Move every pin rigidly: `rotation · (rest − rest_center) + translation`.
    pub fn set_pin_transform(&mut self, translation: Vec3, rotation: Quat) {
        for k in 0..self.pins.len() {
            self.pin_target[k] =
                Some((rotation * (self.pin_rest[k] - self.rest_center) + translation).into());
        }
    }

    pub fn begin_frame(&mut self, p: &Particles) {
        for (k, &i) in self.pins.iter().enumerate() {
            self.pin_frame_start[k] = p.pos[(self.start + i) as usize];
        }
    }

    pub fn end_frame(&mut self) {
        self.pin_target.iter_mut().for_each(|t| *t = None);
    }

    /// Move pinned particles toward their targets; `frac` is the fraction of
    /// the frame done by the end of this substep.
    pub fn move_pins(&self, p: &mut Particles, frac: f32, sub_dt: f32) {
        for (k, &i) in self.pins.iter().enumerate() {
            let Some(target) = self.pin_target[k] else {
                continue;
            };
            let gi = (self.start + i) as usize;
            if p.inv_mass[gi] > 0.0 {
                continue;
            }
            let next = self.pin_frame_start[k].lerp(target, frac);
            p.vel[gi] = (next - p.pos[gi]) / sub_dt;
            p.pos[gi] = next;
            p.pred[gi] = next;
        }
    }

    pub fn reset_lambdas(&mut self) {
        self.distance.reset();
        if let Some(b) = &mut self.bending {
            b.reset();
        }
        if let Some((t, _)) = &mut self.tethers {
            t.reset();
        }
    }

    /// Aerodynamic drag and lift per triangle, `Δx* = dt² w ΣF/3` per vertex
    /// (per-vertex gather over incident triangles, as `cloth/aero.ts`).
    pub fn aero(&mut self, p: &mut Particles, dt: f32) {
        if self.drag == 0.0 && self.lift == 0.0 {
            return;
        }
        let start = self.start as usize;
        let (wind, drag, lift) = (self.wind, self.drag, self.lift);
        let pp = &*p;
        // Each triangle's force once (the reference gathers it at all three
        // vertices, which computes it three times); a third goes to each vertex.
        let force = |tri: &[u32; 3]| -> Vec3A {
            let [a, b, c] = tri.map(|k| start + k as usize);
            let v_rel = (pp.vel[a] + pp.vel[b] + pp.vel[c]) / 3.0 - wind;
            let v2 = v_rel.length_squared();
            if v2 <= 1e-12 {
                return Vec3A::ZERO;
            }
            let n_raw = (pp.pos[b] - pp.pos[a]).cross(pp.pos[c] - pp.pos[a]);
            let n2 = n_raw.length_squared();
            if n2 <= 1e-12 {
                return Vec3A::ZERO;
            }
            let n_mag = n2.sqrt();
            let area = 0.5 * n_mag;
            let n_unit = n_raw / n_mag;
            let n_bar = if n_unit.dot(v_rel) < 0.0 {
                -n_unit
            } else {
                n_unit
            };
            let v_hat = v_rel / v2.sqrt();
            let f_drag = -v_hat * (drag * v2 * area * n_bar.dot(v_hat));
            let f_lift = n_bar.cross(v_hat).cross(v_hat) * (lift * v2 * area);
            (f_drag + f_lift) / 3.0
        };
        let tris = &self.graph.triangles;
        if tris.len() < crate::par::CHEAP_SEQ_THRESHOLD {
            self.tri_force.clear();
            self.tri_force.extend(tris.iter().map(force));
        } else {
            self.tri_force.resize(tris.len(), Vec3A::ZERO);
            self.tri_force
                .par_iter_mut()
                .with_min_len(1024)
                .zip(tris.par_iter())
                .for_each(|(f, t)| *f = force(t));
        }
        let (offsets, list) = (&self.tri_offsets, &self.tri_list);
        let dt2 = dt * dt;
        for v in 0..self.count as usize {
            let w = p.inv_mass[start + v];
            if w <= 0.0 {
                continue;
            }
            let mut f_sum = Vec3A::ZERO;
            for &t in &list[offsets[v] as usize..offsets[v + 1] as usize] {
                f_sum += self.tri_force[t as usize];
            }
            p.pred[start + v] += f_sum * (dt2 * w);
        }
    }

    pub fn solve(&mut self, p: &mut Particles, dt: f32) {
        let Particles { pred, inv_mass, .. } = p;
        self.distance.solve_distance(pred, inv_mass, dt);
        if let Some(b) = &mut self.bending {
            b.solve_bending(pred, inv_mass, dt);
        }
        if let Some((t, anchors)) = &mut self.tethers {
            t.solve_tethers(pred, inv_mass, anchors, dt);
        }
    }

    /// Blend each free particle's velocity toward the mean of its edge neighbors.
    pub fn damp(&mut self, p: &mut Particles) {
        let Some(k) = self.damping else {
            return;
        };
        let start = self.start as usize;
        let (offsets, list) = (&self.nbr_offsets, &self.nbr_list);
        let vel = &p.vel;
        let out = SharedMut::new(&mut self.smoothed[..]);
        crate::par::for_each_range_cheap(self.count as usize, |i| {
            let v = vel[start + i];
            let (a, b) = (offsets[i] as usize, offsets[i + 1] as usize);
            let mut sum = Vec3A::ZERO;
            for &j in &list[a..b] {
                sum += vel[start + j as usize];
            }
            let mean = sum / (b - a).max(1) as f32;
            // SAFETY: index i written only by this task.
            unsafe { *out.get(i) = v.lerp(mean, k) };
        });
        for i in 0..self.count as usize {
            if p.inv_mass[start + i] > 0.0 {
                p.vel[start + i] = self.smoothed[i];
            }
        }
    }

    /// Update wetness from per-particle liquid touch counts over `dt` seconds.
    pub fn update_wetness(&mut self, touches: impl Fn(usize) -> u32, dt: f32) {
        let start = self.start as usize;
        for (i, w) in self.wetness.iter_mut().enumerate() {
            let t = touches(start + i);
            if t > 0 {
                *w = 1.0 - (1.0 - *w) * (-self.wet_rate * dt * t as f32).exp();
            }
            *w = (*w - self.dry_rate * dt).max(0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_modes_select_grid_vertices() {
        let opts = |pins| ClothOptions {
            pins,
            ..Default::default()
        };
        let (cols, rows) = (10, 6);
        assert_eq!(
            opts(ClothPins::Top).pinned_indices(cols, rows),
            (0..11).collect::<Vec<_>>()
        );
        assert_eq!(
            opts(ClothPins::TopCorners).pinned_indices(cols, rows),
            vec![0, 10]
        );
        assert_eq!(
            opts(ClothPins::Corners).pinned_indices(cols, rows),
            vec![0, 10, 66, 76]
        );
        assert!(opts(ClothPins::None).pinned_indices(cols, rows).is_empty());
        // A tent ridge across the middle row.
        let ridge = opts(ClothPins::UvLines(vec![[
            Vec2::new(0.0, 0.5),
            Vec2::new(1.0, 0.5),
        ]]))
        .pinned_indices(cols, rows);
        assert_eq!(ridge, (33..44).collect::<Vec<_>>());
        let centre = opts(ClothPins::Uv(vec![Vec2::splat(0.5)])).pinned_indices(cols, rows);
        assert_eq!(centre, vec![38]);
    }

    #[test]
    fn bend_compliance_matches_reference_formula() {
        // 10^(−1 + 5·0.75) · (30/30)^4 · (0.35/0.1)
        let c = cloth_bend_compliance(0.75, 30, 0.1);
        assert!((c - 10f32.powf(2.75) * 3.5).abs() / c < 1e-5);
    }
}
