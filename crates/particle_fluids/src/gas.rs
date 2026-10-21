//! Smoke: air particles fill the container as a position based fluid, a heat
//! source warms the air so it rises, and massless tracers ride the air's
//! velocity field (port of `gas/GasSystem.ts`, `gas/heat.ts`, `gas/smokeAdvect.ts`).
//!
//! Each substep, after the velocity update:
//! 1. every live tracer ages, retires past its lifetime or outside the bounds,
//!    and otherwise moves at `v(x) = Σ_j v_j W(x − x_j) / Σ_j W(x − x_j)` over
//!    the air particles within `h` (Macklin et al. 2014 eq. 28);
//! 2. air inside the source sphere is set to temperature 1, all air cools by
//!    `exp(−cooling · dt)`, and Boussinesq buoyancy accelerates it upward by
//!    `heat · (T − T̄)`, `T̄` the mean air temperature.
//!
//! Extensions for an outdoor world: the air's horizontal velocity relaxes
//! toward a host-controlled wind; with `wind_tunnel` the container has no side
//! walls and air leaving one side re-enters at the opposite side at ambient
//! temperature, so the plume bends and drifts out; tracers that leave retire.

use crate::grid::HashGrid;
use crate::math::{Aabb, Rng, Sph};
use crate::par::{SharedMut, chunked_sum, for_each_range};
use crate::particles::{Kind, Particles};
use glam::{Vec3, Vec3A};

/// A heated smoke source. Defaults are the reference `Simulation.addSmoke` values.
#[derive(Clone, Debug, PartialEq)]
pub struct SmokeOptions {
    /// Where smoke is released and air is heated. Default: centre of the container floor.
    pub source: Option<Vec3>,
    /// Source radius, m. Default 0.15.
    pub radius: f32,
    /// Tracers released per second. Default 4000.
    pub rate: f32,
    /// Highest `rate` you'll set later; sizes the tracer pool. Default `2 × rate`.
    pub max_rate: Option<f32>,
    /// Seconds each tracer lives. Default 6.
    pub lifetime: f32,
    /// Upward acceleration of air at temperature 1 above the mean, m/s². Default 3.
    pub heat: f32,
    /// Exponential cooling rate, 1/s. Default 0.6.
    pub cooling: f32,
    /// Wind velocity, m/s (host-updated with `set_smoke_wind`).
    pub wind: Vec3,
    /// How fast the air's horizontal velocity relaxes toward the wind, 1/s. Default 1.5.
    pub wind_coupling: f32,
    /// No side walls: air leaving a side re-enters at the opposite side at
    /// ambient temperature. Default true.
    pub wind_tunnel: bool,
    /// XSPH viscosity of the air. Default 0.02.
    pub air_viscosity: f32,
    /// Vorticity confinement of the air. Default 0.06.
    pub air_vorticity: f32,
}

impl Default for SmokeOptions {
    fn default() -> Self {
        Self {
            source: None,
            radius: 0.15,
            rate: 4000.0,
            max_rate: None,
            lifetime: 6.0,
            heat: 3.0,
            cooling: 0.6,
            wind: Vec3::ZERO,
            wind_coupling: 1.5,
            wind_tunnel: true,
            air_viscosity: 0.02,
            air_vorticity: 0.06,
        }
    }
}

/// Live smoke tracers, compact (index order changes as tracers retire).
#[derive(Default)]
pub(crate) struct Tracers {
    pub pos: Vec<Vec3A>,
    pub vel: Vec<Vec3A>,
    pub age: Vec<f32>,
    /// Fixed per-tracer random number in [0, 1), the "grain" weight seed.
    pub seed: Vec<f32>,
}

impl Tracers {
    pub fn len(&self) -> usize {
        self.pos.len()
    }
}

pub(crate) struct Smoke {
    pub fluid: u16,
    pub air_start: usize,
    pub air_count: usize,
    pub source: Vec3,
    pub source_radius: f32,
    pub rate: f32,
    pub max_rate: f32,
    pub lifetime: f32,
    pub heat: f32,
    pub cooling: f32,
    pub wind: Vec3,
    pub wind_coupling: f32,
    pub wind_tunnel: bool,
    pub capacity: usize,
    pub container: Aabb,
    /// Tracers outside this box retire.
    pub bounds: Aabb,
    pub tracers: Tracers,
    pending: Vec<Vec3A>,
    carry: f32,
    pub temperature: Vec<f32>,
    alive_flags: Vec<bool>,
    snapshot: Vec<[Vec3A; 2]>,
}

impl Smoke {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        opts: &SmokeOptions,
        fluid: u16,
        air_start: usize,
        air_count: usize,
        container: Aabb,
        source: Vec3,
    ) -> Self {
        let max_rate = opts.max_rate.unwrap_or(opts.rate * 2.0).max(opts.rate);
        let capacity = (max_rate * opts.lifetime * 1.1).ceil() as usize + 1000;
        let height = container.size().y;
        let top = container.max.y - 0.1 * height;
        let bounds = if opts.wind_tunnel {
            Aabb::new(
                Vec3::new(container.min.x, container.min.y - 1.0, container.min.z),
                Vec3::new(container.max.x, top, container.max.z),
            )
        } else {
            Aabb::new(
                container.min - Vec3::ONE,
                Vec3::new(container.max.x, top, container.max.z),
            )
        };
        Self {
            fluid,
            air_start,
            air_count,
            source,
            source_radius: opts.radius,
            rate: opts.rate,
            max_rate,
            lifetime: opts.lifetime,
            heat: opts.heat,
            cooling: opts.cooling,
            wind: opts.wind,
            wind_coupling: opts.wind_coupling,
            wind_tunnel: opts.wind_tunnel,
            capacity,
            container,
            bounds,
            tracers: Tracers::default(),
            pending: Vec::new(),
            carry: 0.0,
            temperature: vec![0.0; air_count],
            alive_flags: Vec::new(),
            snapshot: Vec::new(),
        }
    }

    /// Queue one tracer; `false` if the pool is full.
    pub fn emit(&mut self, position: Vec3A) -> bool {
        if self.tracers.len() + self.pending.len() >= self.capacity {
            return false;
        }
        self.pending.push(position);
        true
    }

    /// Queue this step's steady emission from the source disc.
    pub fn emit_steady(&mut self, dt: f32, particle_radius: f32, rng: &mut Rng) {
        self.carry += self.rate.min(self.max_rate).max(0.0) * dt;
        let count = self.carry.floor();
        self.carry -= count;
        for _ in 0..count as u32 {
            let angle = rng.next_f32() * std::f32::consts::TAU;
            let r = self.source_radius * 0.9 * rng.next_f32().sqrt();
            let p = Vec3A::new(
                self.source.x + angle.cos() * r,
                self.source.y + particle_radius + rng.next_f32() * self.source_radius * 0.3,
                self.source.z + angle.sin() * r,
            );
            if !self.emit(p) {
                break;
            }
        }
    }

    /// Release queued tracers (the reference's `beforeStep` spawn).
    pub fn spawn(&mut self, rng: &mut Rng) {
        for p in self.pending.drain(..) {
            self.tracers.pos.push(p);
            self.tracers.vel.push(Vec3A::ZERO);
            self.tracers.age.push(0.0);
            self.tracers.seed.push(rng.next_f32());
        }
    }

    /// Advect, age and retire tracers with the air's velocity field.
    pub fn advect_tracers(&mut self, grid: &HashGrid, p: &Particles, sph: &Sph, dt: f32) {
        let n = self.tracers.len();
        self.alive_flags.resize(n, true);
        let (lifetime, bounds) = (self.lifetime, self.bounds);
        // Air position and velocity in the grid's cell order, so each query
        // reads contiguous runs. Non-air particles go far away (zero weight).
        let fluid = self.fluid;
        self.snapshot.clear();
        self.snapshot.extend(grid.sorted.iter().map(|&j| {
            let j = j as usize;
            if p.kind[j] == Kind::Air && p.material[j] == fluid {
                [p.pos[j], p.vel[j]]
            } else {
                [Vec3A::splat(1e30), Vec3A::ZERO]
            }
        }));
        let snapshot = &self.snapshot;
        {
            let Tracers { pos, vel, age, .. } = &mut self.tracers;
            let pos = SharedMut::new(&mut pos[..]);
            let vel = SharedMut::new(&mut vel[..]);
            let age = SharedMut::new(&mut age[..]);
            let alive = SharedMut::new(&mut self.alive_flags[..]);
            for_each_range(n, |s| {
                // SAFETY: tracer s is touched only by this task.
                let (x, v, a, live) = unsafe { (pos.get(s), vel.get(s), age.get(s), alive.get(s)) };
                *a += dt;
                if *a >= lifetime || !bounds.contains(Vec3::from(*x)) {
                    *live = false;
                    return;
                }
                *live = true;
                let at = *x;
                let mut vsum = Vec3A::ZERO;
                let mut wsum = 0.0;
                grid.for_each_run(at, sph.h, |s0, s1| {
                    for [q, qv] in &snapshot[s0..s1] {
                        let w = sph.poly6_r2((at - *q).length_squared());
                        vsum += *qv * w;
                        wsum += w;
                    }
                });
                let velocity = if wsum > 1e-12 {
                    vsum / wsum.max(1e-12)
                } else {
                    Vec3A::ZERO
                };
                *x = at + velocity * dt;
                *v = velocity;
            });
        }
        if self.alive_flags.iter().any(|&a| !a) {
            let flags = std::mem::take(&mut self.alive_flags);
            let t = &mut self.tracers;
            let mut k = 0;
            t.pos.retain(|_| {
                k += 1;
                flags[k - 1]
            });
            k = 0;
            t.vel.retain(|_| {
                k += 1;
                flags[k - 1]
            });
            k = 0;
            t.age.retain(|_| {
                k += 1;
                flags[k - 1]
            });
            k = 0;
            t.seed.retain(|_| {
                k += 1;
                flags[k - 1]
            });
            self.alive_flags = flags;
        }
    }

    /// Heat, cooling, buoyancy, wind coupling and the wind-tunnel wrap.
    pub fn heat_and_wind(&mut self, p: &mut Particles, dt: f32) {
        let (start, n) = (self.air_start, self.air_count);
        let (src, r2) = (
            Vec3A::from(self.source),
            self.source_radius * self.source_radius,
        );
        let decay = (-self.cooling * dt).exp();
        {
            let pos = &p.pos;
            let out = SharedMut::new(&mut self.temperature[..]);
            for_each_range(n, |k| {
                // SAFETY: index k touched only by this task.
                let t = unsafe { out.get(k) };
                if (pos[start + k] - src).length_squared() < r2 {
                    *t = 1.0;
                }
                *t *= decay;
            });
        }
        let temperature = &self.temperature;
        let mean = chunked_sum(n, 0.0f64, |k| temperature[k] as f64) / n.max(1) as f64;
        let mean = mean as f32;
        let lift = self.heat * dt;
        let relax = 1.0 - (-self.wind_coupling.max(0.0) * dt).exp();
        let wind = Vec3A::from(self.wind);
        for (k, &t) in temperature.iter().enumerate() {
            let v = &mut p.vel[start + k];
            v.y += lift * (t - mean);
            if relax > 0.0 {
                v.x += (wind.x - v.x) * relax;
                v.z += (wind.z - v.z) * relax;
            }
        }
        if self.wind_tunnel {
            let (lo, hi) = (
                Vec3A::from(self.container.min),
                Vec3A::from(self.container.max),
            );
            let size = hi - lo;
            for k in 0..n {
                let i = start + k;
                let mut x = p.pos[i];
                let mut wrapped = false;
                if x.x > hi.x {
                    x.x -= size.x;
                    wrapped = true;
                } else if x.x < lo.x {
                    x.x += size.x;
                    wrapped = true;
                }
                if x.z > hi.z {
                    x.z -= size.z;
                    wrapped = true;
                } else if x.z < lo.z {
                    x.z += size.z;
                    wrapped = true;
                }
                if wrapped {
                    p.pos[i] = x;
                    p.pred[i] = x;
                    self.temperature[k] = 0.0;
                }
            }
        }
    }
}
