//! [`Simulation`]: one effect's particles, materials, colliders and solver loop
//! (port of `core/loop.ts` and `simulation/Simulation.ts`).

use crate::cloth::graph::{ClothGraphError, create_cloth_graph, plane_grid};
use crate::cloth::surface::{ClothSurface, fill_surface};
use crate::cloth::{Cloth, ClothOptions};
use crate::colliders::heightfield::Heightfield;
use crate::colliders::primitives::{ColliderOptions, Shape};
use crate::colliders::sdf::{SdfCollider, SdfData};
use crate::colliders::{ColliderSet, PairLambda};
use crate::contact::{CONTACT_RADIUS_EXPANSION, Contacts};
use crate::fluid::{FluidMat, FluidSolver, FusedColliders, LiquidParams};
use crate::gas::{Smoke, SmokeOptions};
use crate::grid::HashGrid;
use crate::math::{Aabb, Rng};
use crate::mesh::TriangleMesh;
use crate::par::{SharedMut, for_each_index};
use crate::particles::{Kind, Particles};
use crate::render::liquid_surface::{
    self, LiquidSurfaceField, LiquidSurfaceOptions, SurfaceInputs,
};
use crate::render::smoke_volume::{self, SmokeVolume, SmokeVolumeOptions};
use crate::softbody::skin::SkinBinding;
use crate::softbody::voxelize::{VoxelShape, voxelize};
use crate::softbody::{SoftbodyOptions, SoftbodySystem};
use crate::stepper::{FrameStepper, StepInfo};
use glam::{Quat, Vec3, Vec3A};
use std::sync::Arc;

macro_rules! timed {
    ($name:expr) => {
        #[cfg(feature = "timing")]
        let _t = timing::Scope::new($name);
    };
}

/// Fixed solver step, s.
pub const FIXED_DT: f32 = 1.0 / 60.0;

/// How particle size is chosen. Every particle in a simulation has the same radius.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParticleSizing {
    /// Radius in metres; the count follows from what is added.
    Radius(f32),
    /// Total particle budget (not counting [`SimConfig::reserve`]); the radius
    /// is the smallest at which everything added fits.
    Budget(usize),
}

/// The box an effect happens in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Container {
    pub bounds: Aabb,
    /// A floor and four side walls keep particles in.
    pub walls: bool,
    /// A lid on top (smoke always gets one).
    pub lid: bool,
    /// Friction of the walls. Default μ_s 0.5, μ_k 0.4.
    pub friction: ColliderOptions,
}

impl Container {
    /// Floor and four walls, open top (the reference default).
    pub fn tank(bounds: Aabb) -> Self {
        Self {
            bounds,
            walls: true,
            lid: false,
            friction: ColliderOptions::default(),
        }
    }

    /// Floor, walls and lid.
    pub fn closed(bounds: Aabb) -> Self {
        Self {
            bounds,
            walls: true,
            lid: true,
            friction: ColliderOptions::default(),
        }
    }

    /// No walls: the box only bounds the effect. Liquid leaving it retires.
    pub fn open(bounds: Aabb) -> Self {
        Self {
            bounds,
            walls: false,
            lid: false,
            friction: ColliderOptions::default(),
        }
    }
}

/// Settings of a [`Simulation`].
#[derive(Clone, Debug, PartialEq)]
pub struct SimConfig {
    pub container: Option<Container>,
    pub sizing: ParticleSizing,
    /// Extra free slots for [`Simulation::emit_liquid`]. Default 0.
    pub reserve: usize,
    /// Substeps per 1/60 s step. `None`: 2–24, chosen like the reference
    /// (`ceil(base · max(1, 0.018 / r))`, base 4 liquid, 2 smoke, 6 soft body, 8 cloth).
    pub substeps: Option<u32>,
    /// Constraint iterations per substep. Default 2.
    pub iterations: u32,
    /// m/s². `None`: (0, −9.81, 0), or (0, −1, 0) with smoke.
    pub gravity: Option<Vec3>,
    /// `(μ_s, μ_k)` between particles. Default (0.3, 0.2); contacts are on
    /// whenever there is cloth or a soft body.
    pub contact_friction: (f32, f32),
    /// Run on a dedicated rayon pool of this many threads. `None`: the global pool.
    pub threads: Option<usize>,
    /// Seed for the smoke emitter and tracer grain.
    pub seed: u64,
    /// Steps per `step()` call at most. Default 4.
    pub max_steps_per_frame: u32,
}

impl SimConfig {
    pub fn new(container: Option<Container>, sizing: ParticleSizing) -> Self {
        Self {
            container,
            sizing,
            reserve: 0,
            substeps: None,
            iterations: 2,
            gravity: None,
            contact_friction: (0.3, 0.2),
            threads: None,
            seed: 0x5eed,
            max_steps_per_frame: 4,
        }
    }
}

/// What fills a liquid at start.
#[derive(Clone, Debug, Default)]
pub enum LiquidFill {
    /// Nothing: the liquid only appears through [`Simulation::emit_liquid`].
    #[default]
    None,
    /// A box (the part inside the container), lattice spacing `2r`.
    Box(Aabb),
    /// A closed mesh in world space.
    Mesh(TriangleMesh),
    /// Explicit particle positions.
    Points(Vec<Vec3>),
}

/// A liquid. Defaults follow the reference `Simulation.addFluid`.
#[derive(Clone, Debug, Default)]
pub struct LiquidOptions {
    pub fill: LiquidFill,
    pub params: LiquidParams,
    /// Retire particles below this height.
    pub kill_height: Option<f32>,
    /// Retire particles that touch the heightfield (rain soaking into terrain).
    pub retire_on_heightfield: bool,
    /// Keep particles that leave the container instead of retiring them.
    pub keep_outside_container: bool,
}

/// Why a liquid particle was retired.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetireReason {
    LeftContainer,
    BelowKillHeight,
    TouchedHeightfield,
}

/// A liquid particle retired during the last `step()` call (for splash decals).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RetiredParticle {
    pub liquid: LiquidHandle,
    pub position: Vec3,
    pub velocity: Vec3,
    pub reason: RetireReason,
}

macro_rules! handle {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub struct $name(pub(crate) u32);
        impl $name {
            /// Index in add order.
            pub fn index(self) -> usize {
                self.0 as usize
            }
        }
    };
}

handle!(
    /// A liquid added with [`Simulation::add_liquid`].
    LiquidHandle
);
handle!(
    /// A cloth added with [`Simulation::add_cloth`].
    ClothHandle
);
handle!(
    /// A soft body added with [`Simulation::add_softbody`].
    SoftbodyHandle
);
handle!(
    /// The smoke source added with [`Simulation::add_smoke`].
    SmokeHandle
);

/// A collider: a primitive shape or a baked mesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColliderHandle {
    Primitive(u32),
    Mesh(u32),
}

/// Errors from building or driving a [`Simulation`].
#[derive(Clone, Debug, PartialEq)]
pub enum SimError {
    /// An `add_*` call after the simulation started.
    AlreadyStarted(&'static str),
    /// Nothing but obstacles was added.
    NothingToSimulate,
    BadSizing,
    SmokeNeedsContainer,
    OneSmokeOnly,
    /// Smoke can't share a simulation with liquids, cloth or soft bodies.
    SmokeWithOtherMaterials,
    /// A liquid fill produced no particles.
    LiquidNoRoom(usize),
    SoftbodyTooSmall(usize),
    CannotFitBudget(usize),
    Cloth(ClothGraphError),
    BadMesh(String),
    /// Every slot is in use.
    NoFreeSlots,
}

impl std::fmt::Display for SimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SimError::AlreadyStarted(what) => write!(
                f,
                "Simulation: {what} must happen before the simulation starts"
            ),
            SimError::NothingToSimulate => write!(
                f,
                "Simulation: add a liquid, smoke, soft body, or cloth before stepping"
            ),
            SimError::BadSizing => {
                write!(f, "Simulation: particle radius or budget must be positive")
            }
            SimError::SmokeNeedsContainer => {
                write!(f, "add_smoke: smoke needs a container for the air to fill")
            }
            SimError::OneSmokeOnly => {
                write!(f, "add_smoke: a simulation can have one smoke source")
            }
            SimError::SmokeWithOtherMaterials => {
                write!(
                    f,
                    "smoke can't share a simulation with liquids, soft bodies or cloth; use a second Simulation"
                )
            }
            SimError::LiquidNoRoom(i) => write!(
                f,
                "add_liquid: liquid #{i} has no room; check its fill and the container"
            ),
            SimError::SoftbodyTooSmall(i) => write!(
                f,
                "add_softbody: soft body #{i} is too small for the particle size"
            ),
            SimError::CannotFitBudget(n) => {
                write!(f, "Simulation: the scene can't fit in {n} particles")
            }
            SimError::Cloth(e) => write!(f, "{e}"),
            SimError::BadMesh(e) => write!(f, "{e}"),
            SimError::NoFreeSlots => write!(f, "emit_liquid: every particle slot is in use"),
        }
    }
}

impl std::error::Error for SimError {}

/// Spacing of cloth particles used for the budget estimate, in radii.
const CLOTH_SPACING: f32 = 2.2;

struct LiquidRuntime {
    kill_height: Option<f32>,
    retire_on_heightfield: bool,
    keep_outside: bool,
}

struct Layout {
    radius: f32,
    soft: Vec<(Vec<Vec3>, usize, Vec<[u32; 2]>)>,
    cloth: Vec<(crate::cloth::graph::ClothGraph, u32, u32)>,
    liquid: Vec<Vec<Vec3>>,
    air: Vec<Vec3>,
}

impl Layout {
    fn count(&self) -> usize {
        self.soft.iter().map(|s| s.0.len()).sum::<usize>()
            + self
                .cloth
                .iter()
                .map(|c| c.0.positions.len())
                .sum::<usize>()
            + self.liquid.iter().map(Vec::len).sum::<usize>()
            + self.air.len()
    }
}

/// One particle simulation (one effect): liquids, cloth, soft bodies or smoke,
/// with their colliders. Add everything, then call [`step`](Self::step) every
/// frame. `Simulation` is `Send`, so it can be stepped on a worker thread.
pub struct Simulation {
    config: SimConfig,
    started: bool,
    radius: f32,
    liquid_specs: Vec<LiquidOptions>,
    cloth_specs: Vec<ClothOptions>,
    soft_specs: Vec<SoftbodyOptions>,
    smoke_spec: Option<SmokeOptions>,
    colliders: ColliderSet,
    collider_lambda: Vec<PairLambda>,
    p: Particles,
    grid: HashGrid,
    fluid: FluidSolver,
    fluids: Vec<FluidMat>,
    liquid_rt: Vec<LiquidRuntime>,
    members_dirty: bool,
    contacts: Option<Contacts>,
    cloths: Vec<Cloth>,
    soft: SoftbodySystem,
    smoke: Option<Smoke>,
    active: Vec<u32>,
    delta: Vec<Vec3A>,
    dv: Vec<Vec3A>,
    touch: Vec<bool>,
    retired: Vec<RetiredParticle>,
    rng: Rng,
    gravity: Vec3,
    substeps: u32,
    stepper: FrameStepper,
    pool: Option<Arc<rayon::ThreadPool>>,
    time: f64,
}

impl Simulation {
    pub fn new(config: SimConfig) -> Self {
        let pool = config
            .threads
            .and_then(|n| {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(n.max(1))
                    .build()
                    .ok()
            })
            .map(Arc::new);
        let radius = match config.sizing {
            ParticleSizing::Radius(r) => r,
            ParticleSizing::Budget(_) => 0.0,
        };
        Self {
            rng: Rng::new(config.seed),
            stepper: FrameStepper::new(FIXED_DT as f64, config.max_steps_per_frame.max(1)),
            gravity: config.gravity.unwrap_or(Vec3::new(0.0, -9.81, 0.0)),
            config,
            started: false,
            radius,
            liquid_specs: Vec::new(),
            cloth_specs: Vec::new(),
            soft_specs: Vec::new(),
            smoke_spec: None,
            colliders: ColliderSet::default(),
            collider_lambda: Vec::new(),
            p: Particles::new(0, 1.0),
            grid: HashGrid::new(1.0),
            fluid: FluidSolver::new(0, 1.0),
            fluids: Vec::new(),
            liquid_rt: Vec::new(),
            members_dirty: false,
            contacts: None,
            cloths: Vec::new(),
            soft: SoftbodySystem::new(0, 1.0),
            smoke: None,
            active: Vec::new(),
            delta: Vec::new(),
            dv: Vec::new(),
            touch: Vec::new(),
            retired: Vec::new(),
            substeps: 1,
            pool,
            time: 0.0,
        }
    }

    // ------------------------------------------------------------------ adding

    fn assert_not_started(&self, what: &'static str) -> Result<(), SimError> {
        if self.started {
            Err(SimError::AlreadyStarted(what))
        } else {
            Ok(())
        }
    }

    /// Add a liquid. With [`LiquidFill::None`] it starts empty and is fed by
    /// [`emit_liquid`](Self::emit_liquid) (rain, fountains, spouts).
    pub fn add_liquid(&mut self, options: LiquidOptions) -> Result<LiquidHandle, SimError> {
        self.assert_not_started("add_liquid")?;
        if self.smoke_spec.is_some() {
            return Err(SimError::SmokeWithOtherMaterials);
        }
        if let LiquidFill::Mesh(m) = &options.fill {
            m.validate().map_err(SimError::BadMesh)?;
        }
        self.liquid_specs.push(options);
        Ok(LiquidHandle(self.liquid_specs.len() as u32 - 1))
    }

    /// Add a rectangle of cloth.
    pub fn add_cloth(&mut self, options: ClothOptions) -> Result<ClothHandle, SimError> {
        self.assert_not_started("add_cloth")?;
        if self.smoke_spec.is_some() {
            return Err(SimError::SmokeWithOtherMaterials);
        }
        self.cloth_specs.push(options);
        Ok(ClothHandle(self.cloth_specs.len() as u32 - 1))
    }

    /// Turn a closed mesh (world space) into a soft body.
    pub fn add_softbody(&mut self, options: SoftbodyOptions) -> Result<SoftbodyHandle, SimError> {
        self.assert_not_started("add_softbody")?;
        if self.smoke_spec.is_some() {
            return Err(SimError::SmokeWithOtherMaterials);
        }
        options.mesh.validate().map_err(SimError::BadMesh)?;
        self.soft_specs.push(options);
        Ok(SoftbodyHandle(self.soft_specs.len() as u32 - 1))
    }

    /// Add a heated smoke source. Air fills the whole container.
    pub fn add_smoke(&mut self, options: SmokeOptions) -> Result<SmokeHandle, SimError> {
        self.assert_not_started("add_smoke")?;
        if self.config.container.is_none() {
            return Err(SimError::SmokeNeedsContainer);
        }
        if self.smoke_spec.is_some() {
            return Err(SimError::OneSmokeOnly);
        }
        if !self.liquid_specs.is_empty()
            || !self.cloth_specs.is_empty()
            || !self.soft_specs.is_empty()
        {
            return Err(SimError::SmokeWithOtherMaterials);
        }
        if self.config.gravity.is_none() {
            self.gravity = Vec3::new(0.0, -1.0, 0.0);
        }
        self.smoke_spec = Some(options);
        Ok(SmokeHandle(0))
    }

    fn add_shape(&mut self, shape: Shape, opts: ColliderOptions) -> ColliderHandle {
        ColliderHandle::Primitive(self.colliders.add_primitive(shape, opts) as u32)
    }

    /// An infinite plane through `point`; particles stay on the side `normal` points to.
    pub fn add_plane(
        &mut self,
        normal: Vec3,
        point: Vec3,
        opts: ColliderOptions,
    ) -> ColliderHandle {
        self.add_shape(
            Shape::Plane {
                normal: normal.normalize_or(Vec3::Y),
                point,
            },
            opts,
        )
    }

    pub fn add_sphere(
        &mut self,
        center: Vec3,
        radius: f32,
        opts: ColliderOptions,
    ) -> ColliderHandle {
        self.add_shape(
            Shape::Sphere {
                center,
                radius: radius.max(1e-6),
            },
            opts,
        )
    }

    /// An oriented box with `half_extents` along the axes of `rotation`.
    pub fn add_box(
        &mut self,
        center: Vec3,
        half_extents: Vec3,
        rotation: Quat,
        opts: ColliderOptions,
    ) -> ColliderHandle {
        self.add_shape(
            Shape::Box {
                center,
                half_extents: half_extents.max(Vec3::splat(1e-6)),
                rotation: rotation.normalize(),
            },
            opts,
        )
    }

    pub fn add_capsule(
        &mut self,
        a: Vec3,
        b: Vec3,
        radius: f32,
        opts: ColliderOptions,
    ) -> ColliderHandle {
        self.add_shape(
            Shape::Capsule {
                a,
                b,
                radius: radius.max(1e-6),
            },
            opts,
        )
    }

    /// A baked mesh obstacle (see [`crate::bake_mesh_to_sdf`]) placed with a
    /// rigid transform and uniform scale. `thickness` adds contact distance.
    #[allow(clippy::too_many_arguments)]
    pub fn add_sdf_collider(
        &mut self,
        sdf: SdfData,
        position: Vec3,
        rotation: Quat,
        scale: f32,
        thickness: Option<f32>,
        opts: ColliderOptions,
    ) -> ColliderHandle {
        // `None`: the reference's cloth thickness, resolved when the simulation starts.
        let thickness = match thickness {
            Some(t) => t.max(0.0),
            None if self.started => self.cloth_thickness(),
            None => -1.0,
        };
        self.colliders.sdfs.push(SdfCollider {
            sdf,
            position,
            rotation: rotation.normalize(),
            scale: scale.max(1e-6),
            thickness,
            mu_s: opts.mu_s,
            mu_k: opts.mu_k,
            enabled: true,
        });
        self.colliders.version += 1;
        ColliderHandle::Mesh(self.colliders.sdfs.len() as u32 - 1)
    }

    /// Move a collider to `position` with `rotation` by the end of the next
    /// frame. The shape is swept across the substeps and drags particles by
    /// friction (`PrimitiveSet.attach`). `rotation` is relative to the shape as
    /// added, about its pivot (plane point, centre, or capsule midpoint). Call
    /// it every frame while the collider moves.
    pub fn move_collider(&mut self, handle: ColliderHandle, position: Vec3, rotation: Quat) {
        match handle {
            ColliderHandle::Primitive(i) => self
                .colliders
                .move_primitive(i as usize, position, rotation),
            ColliderHandle::Mesh(i) => {
                self.set_sdf_transform(ColliderHandle::Mesh(i), position, rotation, None)
            }
        }
    }

    /// Place a collider immediately, with no velocity.
    pub fn teleport_collider(&mut self, handle: ColliderHandle, position: Vec3, rotation: Quat) {
        match handle {
            ColliderHandle::Primitive(i) => self
                .colliders
                .teleport_primitive(i as usize, position, rotation),
            ColliderHandle::Mesh(i) => {
                self.set_sdf_transform(ColliderHandle::Mesh(i), position, rotation, None)
            }
        }
    }

    /// Place a mesh collider (it jumps; friction treats it as static).
    pub fn set_sdf_transform(
        &mut self,
        handle: ColliderHandle,
        position: Vec3,
        rotation: Quat,
        scale: Option<f32>,
    ) {
        if let ColliderHandle::Mesh(i) = handle
            && let Some(s) = self.colliders.sdfs.get_mut(i as usize)
        {
            s.position = position;
            s.rotation = rotation.normalize();
            if let Some(scale) = scale {
                s.scale = scale.max(1e-6);
            }
            self.colliders.version += 1;
        }
    }

    /// Switch a collider on or off.
    pub fn set_collider_enabled(&mut self, handle: ColliderHandle, enabled: bool) {
        match handle {
            ColliderHandle::Primitive(i) => {
                if let Some(p) = self.colliders.prims.get_mut(i as usize) {
                    p.enabled = enabled;
                }
            }
            ColliderHandle::Mesh(i) => {
                if let Some(s) = self.colliders.sdfs.get_mut(i as usize) {
                    s.enabled = enabled;
                }
            }
        }
        self.colliders.version += 1;
    }

    /// Set (or clear) the terrain heightfield.
    pub fn set_heightfield(&mut self, heightfield: Option<Heightfield>) {
        self.colliders.heightfield = heightfield;
        self.colliders.version += 1;
    }

    pub fn heightfield(&self) -> Option<&Heightfield> {
        self.colliders.heightfield.as_ref()
    }

    /// Extra SDF contact distance when the scene has cloth (`1.3 · 2.2 r − r`),
    /// so thin mesh features can't slip between cloth particles.
    fn cloth_thickness(&self) -> f32 {
        if self.cloths.is_empty() {
            0.0
        } else {
            1.3 * CLOTH_SPACING * self.radius - self.radius
        }
    }

    // ----------------------------------------------------------------- building

    /// Build everything added so far. The first [`step`](Self::step) calls it.
    pub fn start(&mut self) -> Result<(), SimError> {
        if self.started {
            return Ok(());
        }
        if self.liquid_specs.is_empty()
            && self.cloth_specs.is_empty()
            && self.soft_specs.is_empty()
            && self.smoke_spec.is_none()
        {
            return Err(SimError::NothingToSimulate);
        }
        let layout = match self.config.sizing {
            ParticleSizing::Radius(r) if r > 0.0 && r.is_finite() => self.layout(r)?,
            ParticleSizing::Budget(n) if n > 0 => self.fit_budget(n)?,
            _ => return Err(SimError::BadSizing),
        };
        for (i, spec) in self.liquid_specs.iter().enumerate() {
            if !matches!(spec.fill, LiquidFill::None) && layout.liquid[i].is_empty() {
                return Err(SimError::LiquidNoRoom(i));
            }
        }
        for (i, s) in layout.soft.iter().enumerate() {
            if s.0.is_empty() {
                return Err(SimError::SoftbodyTooSmall(i));
            }
        }
        self.build(layout);
        self.started = true;
        Ok(())
    }

    /// Lay out every particle at radius `r`: soft bodies, cloth, liquid, air.
    fn layout(&self, r: f32) -> Result<Layout, SimError> {
        let spacing = 2.0 * r;
        let container = self.config.container.map(|c| c.bounds);
        let mut solid_points: Vec<Vec3> = Vec::new();
        let mut soft = Vec::new();
        for spec in &self.soft_specs {
            let v = voxelize(VoxelShape::Mesh(&spec.mesh), r, true);
            solid_points.extend_from_slice(&v.positions);
            soft.push((v.positions, v.surface_count, v.edges));
        }
        let mut cloth = Vec::new();
        for spec in &self.cloth_specs {
            let (cols, rows) = spec.grid_size(r);
            let (verts, idx) = plane_grid(
                spec.width,
                spec.height,
                cols,
                rows,
                spec.position,
                spec.rotation,
            );
            let pins = spec.pinned_indices(cols, rows);
            let graph =
                create_cloth_graph(&verts, &idx, spec.weight, &pins).map_err(SimError::Cloth)?;
            solid_points.extend_from_slice(&graph.positions);
            cloth.push((graph, cols + 1, rows + 1));
        }
        let solid_grid = PointGrid::new(&solid_points, spacing);
        let mut liquid = Vec::new();
        for spec in &self.liquid_specs {
            let points = match &spec.fill {
                LiquidFill::None => Vec::new(),
                LiquidFill::Points(p) => p.clone(),
                LiquidFill::Box(b) => {
                    let b = match container {
                        Some(c) => b.intersect(&c.expanded(-r)),
                        None => *b,
                    };
                    fill_box(&b, spacing)
                }
                LiquidFill::Mesh(m) => voxelize(VoxelShape::Mesh(m), r, false).positions,
            };
            let kept = if matches!(spec.fill, LiquidFill::Points(_)) {
                points
            } else {
                points
                    .into_iter()
                    .filter(|&p| {
                        self.colliders.nearest_distance(Vec3A::from(p), false) >= spacing
                            && !solid_grid.any_within(p, spacing)
                    })
                    .collect()
            };
            liquid.push(kept);
        }
        let mut air = Vec::new();
        if self.smoke_spec.is_some() {
            let c = container.expect("smoke has a container");
            air = fill_box(&c.expanded(-r), spacing);
            if let Some(hf) = &self.colliders.heightfield {
                air.retain(|p| p.y - hf.height(p.x, p.z) >= r);
            }
        }
        Ok(Layout {
            radius: r,
            soft,
            cloth,
            liquid,
            air,
        })
    }

    /// The layout with the smallest radius that fits `budget` (reference `fitBudget`).
    fn fit_budget(&self, budget: usize) -> Result<Layout, SimError> {
        let container = self.config.container.map(|c| c.bounds);
        let mut volume = 0.0f32;
        let mut fixed = 0usize;
        for spec in &self.liquid_specs {
            match &spec.fill {
                LiquidFill::Box(b) => volume += container.map_or(*b, |c| b.intersect(&c)).volume(),
                LiquidFill::Mesh(m) => {
                    if let Some((lo, hi)) = m.bounds() {
                        volume += Aabb::new(lo, hi).volume();
                    }
                }
                LiquidFill::Points(p) => fixed += p.len(),
                LiquidFill::None => {}
            }
        }
        for spec in &self.soft_specs {
            if let Some((lo, hi)) = spec.mesh.bounds() {
                volume += Aabb::new(lo, hi).volume();
            }
        }
        if self.smoke_spec.is_some() {
            volume += container.map_or(0.0, |c| c.volume());
        }
        let area: f32 = self
            .cloth_specs
            .iter()
            .map(|c| c.width * c.height * (CLOTH_SPACING / c.spacing).powi(2))
            .sum();
        let target = budget.saturating_sub(fixed).max(1);
        let estimate = |r: f32| volume / (2.0 * r).powi(3) + area / (CLOTH_SPACING * r).powi(2);
        let (mut lo, mut hi) = (1e-5f32, 10.0f32);
        for _ in 0..80 {
            let mid = (lo * hi).sqrt();
            if estimate(mid) > target as f32 {
                lo = mid
            } else {
                hi = mid
            }
        }
        let mut r = hi;
        let mut best: Option<Layout> = None;
        for _ in 0..12 {
            let layout = self.layout(r)?;
            let count = layout.count();
            if count <= budget && best.as_ref().is_none_or(|b| count > b.count()) {
                best = Some(layout);
            }
            if count <= budget && count as f32 >= 0.97 * budget as f32 {
                break;
            }
            r *= (count.max(1) as f32 / (0.985 * budget as f32)).cbrt();
        }
        let mut tries = 0;
        while best.is_none() && tries < 100 {
            r *= 1.05;
            let layout = self.layout(r)?;
            if layout.count() <= budget {
                best = Some(layout);
            }
            tries += 1;
        }
        best.ok_or(SimError::CannotFitBudget(budget))
    }

    fn build(&mut self, layout: Layout) {
        let r = layout.radius;
        self.radius = r;
        let spacing = 2.0 * r;
        let capacity = layout.count() + self.config.reserve;
        let mut p = Particles::new(capacity, r);
        let mut next_group = 1u32;
        let mut boundary_ranges: Vec<std::ops::Range<u32>> = Vec::new();
        // Soft bodies.
        let mut soft = SoftbodySystem::new(capacity, r);
        for (k, (positions, surface, edges)) in layout.soft.into_iter().enumerate() {
            let spec = &self.soft_specs[k];
            let start = p.alloc_range(positions.len()).expect("capacity");
            let inv = 1.0 / (spec.density * spacing.powi(3));
            let group = next_group;
            next_group += 1;
            for (j, pos) in positions.iter().enumerate() {
                p.init(
                    start + j as u32,
                    Kind::Softbody,
                    k as u16,
                    Vec3A::from(*pos),
                    Vec3A::ZERO,
                    inv,
                    group,
                );
            }
            boundary_ranges.push(start..start + surface as u32);
            soft.add_body(
                &p,
                start,
                positions.len() as u32,
                surface as u32,
                edges,
                spec.shape_matching,
                spec.softness,
                spec.compliance,
            );
        }
        soft.finalize();
        // Cloth.
        let mut cloths = Vec::new();
        for (k, (graph, cols, rows)) in layout.cloth.into_iter().enumerate() {
            let spec = &self.cloth_specs[k];
            let n = graph.positions.len();
            let start = p.alloc_range(n).expect("capacity");
            let group = if spec.self_collision {
                0
            } else {
                next_group += 1;
                next_group - 1
            };
            for (j, pos) in graph.positions.iter().enumerate() {
                p.init(
                    start + j as u32,
                    Kind::Cloth,
                    k as u16,
                    Vec3A::from(*pos),
                    Vec3A::ZERO,
                    graph.inv_mass[j],
                    group,
                );
            }
            boundary_ranges.push(start..start + n as u32);
            cloths.push(Cloth::new(start, cols, rows, graph, spec));
        }
        // Liquids.
        let mut fluids = Vec::new();
        for (k, points) in layout.liquid.into_iter().enumerate() {
            let spec = &self.liquid_specs[k];
            let mut mat = FluidMat::new(Kind::Liquid, spec.params.clone(), spacing);
            let inv = 1.0 / mat.mass;
            if !points.is_empty() {
                let start = p.alloc_range(points.len()).expect("capacity");
                for (j, pos) in points.iter().enumerate() {
                    p.init(
                        start + j as u32,
                        Kind::Liquid,
                        k as u16,
                        Vec3A::from(*pos),
                        Vec3A::ZERO,
                        inv,
                        0,
                    );
                    mat.members.push(start + j as u32);
                }
            }
            fluids.push(mat);
            self.liquid_rt.push(LiquidRuntime {
                kill_height: spec.kill_height,
                retire_on_heightfield: spec.retire_on_heightfield,
                keep_outside: spec.keep_outside_container,
            });
        }
        // Air.
        let mut periodic = None;
        if let Some(opts) = self.smoke_spec.clone() {
            let params = LiquidParams {
                viscosity: opts.air_viscosity,
                vorticity: opts.air_vorticity,
                surface_tension: 0.0,
                adhesion: 0.0,
                ..LiquidParams::default()
            };
            let mut mat = FluidMat::new(Kind::Air, params, spacing);
            let index = fluids.len() as u16;
            let inv = 1.0 / mat.mass;
            let start = p.alloc_range(layout.air.len()).expect("capacity");
            for (j, pos) in layout.air.iter().enumerate() {
                p.init(
                    start + j as u32,
                    Kind::Air,
                    index,
                    Vec3A::from(*pos),
                    Vec3A::ZERO,
                    inv,
                    0,
                );
                mat.members.push(start + j as u32);
            }
            fluids.push(mat);
            let container = self.config.container.expect("smoke has a container").bounds;
            let source = opts.source.unwrap_or(Vec3::new(
                container.center().x,
                container.min.y,
                container.center().z,
            ));
            self.smoke = Some(Smoke::new(
                &opts,
                index,
                start as usize,
                layout.air.len(),
                container,
                source,
            ));
            if opts.wind_tunnel {
                periodic = Some(crate::fluid::Periodic {
                    min: Vec3A::from(container.min),
                    size: Vec3A::from(container.size()),
                });
            }
        }
        // Boundaries: soft-body surfaces and cloths, when there is liquid.
        let mut boundary = Vec::new();
        if !fluids.is_empty() {
            for (id, range) in boundary_ranges.iter().enumerate() {
                for i in range.clone() {
                    p.boundary_id[i as usize] = id as u32;
                    boundary.push(i);
                }
            }
        }
        let h = 2.0 * spacing;
        let mut fluid = FluidSolver::new(capacity, h);
        for f in &fluids {
            for &i in &f.members {
                fluid.reset_particle(i as usize, f.params.rest_density);
            }
        }
        fluid.refresh_rows(&fluids, &boundary);
        fluid.periodic = periodic;
        // Container walls.
        if let Some(c) = self.config.container {
            let wind_tunnel = self.smoke_spec.as_ref().is_some_and(|s| s.wind_tunnel);
            let smoke = self.smoke_spec.is_some();
            if c.walls || smoke {
                let (lo, hi) = (c.bounds.min, c.bounds.max);
                let d = c.friction;
                self.colliders.add_primitive(
                    Shape::Plane {
                        normal: Vec3::Y,
                        point: lo,
                    },
                    d,
                );
                if !wind_tunnel {
                    self.colliders.add_primitive(
                        Shape::Plane {
                            normal: Vec3::X,
                            point: lo,
                        },
                        d,
                    );
                    self.colliders.add_primitive(
                        Shape::Plane {
                            normal: -Vec3::X,
                            point: hi,
                        },
                        d,
                    );
                    self.colliders.add_primitive(
                        Shape::Plane {
                            normal: Vec3::Z,
                            point: lo,
                        },
                        d,
                    );
                    self.colliders.add_primitive(
                        Shape::Plane {
                            normal: -Vec3::Z,
                            point: hi,
                        },
                        d,
                    );
                }
                if c.lid || smoke {
                    self.colliders.add_primitive(
                        Shape::Plane {
                            normal: -Vec3::Y,
                            point: hi,
                        },
                        d,
                    );
                }
            }
        }
        let cloth_thickness = if cloths.is_empty() {
            0.0
        } else {
            1.3 * CLOTH_SPACING * r - r
        };
        for s in &mut self.colliders.sdfs {
            if s.thickness < 0.0 {
                s.thickness = cloth_thickness;
            }
        }
        let needs_contact = !cloths.is_empty() || !soft.bodies.is_empty();
        let (mu_s, mu_k) = self.config.contact_friction;
        self.contacts = needs_contact.then(|| Contacts::new(capacity, mu_s, mu_k));
        let base = [
            if self.liquid_specs.is_empty() { 0 } else { 4 },
            if self.smoke_spec.is_some() { 2 } else { 0 },
            if soft.bodies.is_empty() { 0 } else { 6 },
            if cloths.is_empty() { 0 } else { 8 },
        ]
        .into_iter()
        .max()
        .unwrap_or(1) as f32;
        self.substeps = self
            .config
            .substeps
            .unwrap_or_else(|| (base * (0.018 / r).max(1.0)).ceil().min(24.0) as u32)
            .max(1);
        let cell = if fluids.is_empty() {
            2.0 * r * CONTACT_RADIUS_EXPANSION
        } else {
            h
        };
        self.grid = HashGrid::new(cell);
        self.delta = vec![Vec3A::ZERO; capacity];
        self.dv = vec![Vec3A::ZERO; capacity];
        self.touch = vec![false; capacity];
        p.active_indices(&mut self.active);
        self.p = p;
        self.fluid = fluid;
        self.fluids = fluids;
        self.cloths = cloths;
        self.soft = soft;
    }

    // ----------------------------------------------------------------- stepping

    /// Run `f` on a pool worker thread, so the many short parallel passes of a
    /// step fork and join without waking a sleeping caller each time.
    fn with_pool<R: Send>(&mut self, f: impl FnOnce(&mut Self) -> R + Send) -> R {
        match self.pool.clone() {
            Some(pool) => pool.install(|| f(self)),
            None => {
                let mut out = None;
                rayon::scope(|_| out = Some(f(self)));
                out.expect("scope ran")
            }
        }
    }

    /// Advance by `elapsed` seconds of real time: as many fixed 1/60 s steps as
    /// fit, at most `max_steps_per_frame`. An `elapsed` above 0.25 s counts as
    /// a pause and runs nothing. Builds the simulation on the first call.
    pub fn step(&mut self, elapsed: f32) -> Result<StepInfo, SimError> {
        self.start()?;
        self.retired.clear();
        let info = self.stepper.pump(elapsed as f64);
        if info.steps > 0 {
            self.with_pool(|s| s.run_frame(FIXED_DT, info.steps));
        }
        Ok(info)
    }

    /// Advance exactly one step of `dt` seconds, whatever the clock says (for
    /// tests and recordings). Use a constant `dt` such as [`FIXED_DT`].
    pub fn advance(&mut self, dt: f32) -> Result<(), SimError> {
        self.start()?;
        self.retired.clear();
        if dt > 0.0 && dt.is_finite() {
            self.with_pool(|s| s.run_frame(dt, 1));
        }
        Ok(())
    }

    fn run_frame(&mut self, dt: f32, steps: u32) {
        self.colliders.begin_frame();
        for c in &mut self.cloths {
            c.begin_frame(&self.p);
        }
        for k in 0..steps {
            self.fixed_step(dt, k, steps);
        }
        self.colliders.end_frame();
        for c in &mut self.cloths {
            c.end_frame();
        }
    }

    fn refresh_members(&mut self) {
        for f in &mut self.fluids {
            f.members.sort_unstable();
        }
        let boundary = std::mem::take(&mut self.fluid.boundary_rows);
        self.fluid.refresh_rows(&self.fluids, &boundary);
        self.p.active_indices(&mut self.active);
        self.p.sort_free();
        self.members_dirty = false;
    }

    fn fixed_step(&mut self, dt: f32, k: u32, n: u32) {
        if self.members_dirty {
            self.refresh_members();
        }
        let substeps = self.substeps.max(1);
        let sub = dt / substeps as f32;
        self.colliders.update(dt, k, n);
        if let Some(smoke) = &mut self.smoke {
            smoke.emit_steady(dt, self.radius, &mut self.rng);
            smoke.spawn(&mut self.rng);
        }
        self.colliders.frame_start_clock(sub, substeps);
        let slots = self.colliders.slots();
        let cap = self.p.capacity();
        if self.collider_lambda.len() != cap * slots {
            self.collider_lambda = vec![[0.0; 2]; cap * slots];
        }
        let any_fluid = self.fluids.iter().any(|f| !f.members.is_empty());
        // Every particle is a fluid particle: collider corrections and friction
        // can run inside the fluid passes (friction only when no implicit
        // viscosity solve has to come between them).
        let fused = any_fluid
            && self.contacts.is_none()
            && self.cloths.is_empty()
            && self.soft.bodies.is_empty();
        let fused_post = fused && self.fluids.iter().all(|f| f.params.thickness.is_none());
        for s in 0..substeps {
            let frac = (k as f32 + (s + 1) as f32 / substeps as f32) / n as f32;
            {
                timed!("predict");
                for c in &self.cloths {
                    c.move_pins(&mut self.p, frac, sub);
                }
                self.predict(sub);
                self.soft.predict_rotations(&mut self.p, sub);
            }
            // Pre-solve.
            {
                timed!("grid");
                self.grid.rebuild(&self.p.pred, &self.active);
            }
            if any_fluid {
                timed!("neighbor lists");
                self.fluid.build_lists(&self.grid, &self.p);
            }
            if let Some(contacts) = &mut self.contacts {
                timed!("contacts generate");
                contacts.generate(
                    &self.grid,
                    &self.p,
                    &self.active,
                    any_fluid.then_some(&self.fluid.nbrs),
                );
                contacts.stabilize(&mut self.p, &self.active, &mut self.delta);
            }
            self.collider_lambda.iter_mut().for_each(|l| *l = [0.0; 2]);
            if any_fluid {
                timed!("fluid presolve");
                if !self.fluid.boundary_rows.is_empty() {
                    self.fluid.boundary_volumes(&self.grid, &mut self.p);
                }
                self.fluid.surface_forces(&self.fluids, &mut self.p, sub);
            }
            {
                timed!("solid presolve");
                self.soft.reset_lambdas();
                for c in &mut self.cloths {
                    c.reset_lambdas();
                    c.aero(&mut self.p, sub);
                }
            }
            // Solve. With fluids only, collider work rides along in the fluid passes.
            for _ in 0..self.config.iterations {
                if fused {
                    timed!("fluid density");
                    let fc = FusedColliders {
                        colliders: &self.colliders,
                        lambda: SharedMut::new(&mut self.collider_lambda[..]),
                        touch: SharedMut::new(&mut self.touch[..]),
                        slots,
                        radius: self.p.radius,
                    };
                    let walls = (!self.colliders.is_empty()).then_some(&self.colliders);
                    self.fluid
                        .solve_density(&self.fluids, &mut self.p, sub, walls, Some(&fc));
                    continue;
                }
                if any_fluid {
                    timed!("fluid density");
                    self.fluid.solve_density(
                        &self.fluids,
                        &mut self.p,
                        sub,
                        (!self.colliders.is_empty()).then_some(&self.colliders),
                        None,
                    );
                }
                {
                    timed!("softbody solve");
                    self.soft.solve(&mut self.p, sub);
                }
                {
                    timed!("cloth solve");
                    for c in &mut self.cloths {
                        c.solve(&mut self.p, sub);
                    }
                }
                timed!("contacts+colliders");
                self.solve_contacts_and_colliders(sub, slots);
            }
            {
                timed!("advect");
                self.advect(sub);
            }
            // Post-solve: gas, fluids, soft bodies, cloth.
            if let Some(smoke) = &mut self.smoke {
                timed!("smoke");
                smoke.advect_tracers(&self.grid, &self.p, &self.fluid.sph, sub);
                smoke.heat_and_wind(&mut self.p, sub);
            }
            if any_fluid {
                timed!("fluid post");
                let fc = fused_post.then(|| FusedColliders {
                    colliders: &self.colliders,
                    lambda: SharedMut::new(&mut self.collider_lambda[..]),
                    touch: SharedMut::new(&mut self.touch[..]),
                    slots,
                    radius: self.p.radius,
                });
                self.fluid
                    .velocity_post(&self.fluids, &mut self.p, sub, fc.as_ref());
                for f in &self.fluids {
                    if f.params.thickness.is_some() {
                        self.fluid.viscosity_solve(f, &mut self.p, sub);
                    }
                }
            }
            {
                timed!("solid post");
                if !self.soft.bodies.is_empty() {
                    self.soft.advect_rotations(&mut self.p, sub);
                    self.soft.fit_frames(&self.p, false);
                }
                for c in &mut self.cloths {
                    c.damp(&mut self.p);
                }
            }
            if !fused_post {
                timed!("friction");
                self.friction(sub, slots);
            }
            self.colliders.tick(sub);
        }
        self.time += dt as f64;
        self.update_wetness(dt);
        self.retire_liquid();
    }

    fn predict(&mut self, dt: f32) {
        let g = Vec3A::from(self.gravity) * dt;
        let Particles {
            pos,
            pred,
            vel,
            inv_mass,
            ..
        } = &mut self.p;
        let pred = SharedMut::new(&mut pred[..]);
        let vel = SharedMut::new(&mut vel[..]);
        let (pos, inv_mass) = (&*pos, &*inv_mass);
        crate::par::for_each_index_cheap(&self.active, |i| {
            if inv_mass[i] > 0.0 {
                // SAFETY: index i touched only by this task.
                unsafe {
                    let v = vel.get(i);
                    *v += g;
                    *pred.get(i) = pos[i] + *v * dt;
                }
            }
        });
    }

    fn advect(&mut self, dt: f32) {
        let Particles {
            pos,
            pred,
            vel,
            inv_mass,
            ..
        } = &mut self.p;
        let pos = SharedMut::new(&mut pos[..]);
        let vel = SharedMut::new(&mut vel[..]);
        let (pred, inv_mass) = (&*pred, &*inv_mass);
        let inv_dt = 1.0 / dt;
        crate::par::for_each_index_cheap(&self.active, |i| {
            if inv_mass[i] > 0.0 {
                // SAFETY: index i touched only by this task.
                unsafe {
                    let x = pos.get(i);
                    *vel.get(i) = (pred[i] - *x) * inv_dt;
                    *x = pred[i];
                }
            }
        });
    }

    fn solve_contacts_and_colliders(&mut self, dt: f32, slots: usize) {
        let has_contacts = self.contacts.is_some();
        let has_colliders = !self.colliders.is_empty();
        if !has_contacts && !has_colliders {
            return;
        }
        if let Some(c) = &mut self.contacts {
            c.solve(&self.p, &self.active, &mut self.delta);
        } else {
            for &i in &self.active {
                self.delta[i as usize] = Vec3A::ZERO;
            }
        }
        if has_colliders {
            let p = &self.p;
            let colliders = &self.colliders;
            let radius = p.radius;
            let delta = SharedMut::new(&mut self.delta[..]);
            let lambda = SharedMut::new(&mut self.collider_lambda[..]);
            let touch = SharedMut::new(&mut self.touch[..]);
            for_each_index(&self.active, |i| {
                let w = p.inv_mass[i];
                if w <= 0.0 {
                    return;
                }
                // SAFETY: particle i's delta, lambda row and touch flag are touched only by this task.
                unsafe {
                    let row = lambda.slice(i * slots, slots);
                    let corr = colliders.solve_particle(
                        p.pred[i],
                        p.pos[i],
                        w,
                        radius,
                        dt,
                        row,
                        touch.get(i),
                    );
                    *delta.get(i) += corr;
                }
            });
        }
        for &i in &self.active {
            let i = i as usize;
            self.p.pred[i] += self.delta[i];
        }
    }

    fn friction(&mut self, dt: f32, slots: usize) {
        let has_colliders = !self.colliders.is_empty();
        if let Some(c) = &self.contacts {
            c.friction(&self.p, &self.active, dt, &mut self.dv);
        } else if has_colliders {
            for &i in &self.active {
                self.dv[i as usize] = Vec3A::ZERO;
            }
        } else {
            return;
        }
        if has_colliders {
            let p = &self.p;
            let colliders = &self.colliders;
            let lambda = &self.collider_lambda;
            let dv = SharedMut::new(&mut self.dv[..]);
            for_each_index(&self.active, |i| {
                let w = p.inv_mass[i];
                if w <= 0.0 {
                    return;
                }
                let add = colliders.friction_particle(
                    p.pred[i],
                    p.vel[i],
                    w,
                    dt,
                    &lambda[i * slots..(i + 1) * slots],
                );
                // SAFETY: index i written only by this task.
                unsafe { *dv.get(i) += add };
            });
        }
        for &i in &self.active {
            let i = i as usize;
            self.p.vel[i] += self.dv[i];
        }
    }

    fn update_wetness(&mut self, dt: f32) {
        if self.cloths.is_empty() {
            return;
        }
        let r2 = (3.0 * self.radius).powi(2);
        let any_liquid = self
            .fluids
            .iter()
            .any(|f| f.kind == Kind::Liquid && !f.members.is_empty());
        let (fluid, p) = (&self.fluid, &self.p);
        for c in &mut self.cloths {
            c.update_wetness(
                |i| {
                    if any_liquid {
                        fluid.liquid_touches(p, i, r2)
                    } else {
                        0
                    }
                },
                dt,
            );
        }
    }

    fn retire_liquid(&mut self) {
        let container = self.config.container.map(|c| c.bounds);
        let mut any = false;
        for (l, f) in self.fluids.iter_mut().enumerate() {
            if f.kind != Kind::Liquid {
                continue;
            }
            let rt = &self.liquid_rt[l];
            let p = &mut self.p;
            let touch = &mut self.touch;
            let retired = &mut self.retired;
            f.members.retain(|&i| {
                let iu = i as usize;
                let x = Vec3::from(p.pos[iu]);
                let reason = if !rt.keep_outside && container.is_some_and(|c| !c.contains(x)) {
                    Some(RetireReason::LeftContainer)
                } else if rt.kill_height.is_some_and(|h| x.y < h) {
                    Some(RetireReason::BelowKillHeight)
                } else if rt.retire_on_heightfield && touch[iu] {
                    Some(RetireReason::TouchedHeightfield)
                } else {
                    None
                };
                touch[iu] = false;
                match reason {
                    Some(reason) => {
                        retired.push(RetiredParticle {
                            liquid: LiquidHandle(l as u32),
                            position: x,
                            velocity: Vec3::from(p.vel[iu]),
                            reason,
                        });
                        p.release(i);
                        any = true;
                        false
                    }
                    None => true,
                }
            });
        }
        if any {
            self.members_dirty = true;
            self.refresh_members();
        }
    }

    // ------------------------------------------------------------ liquid control

    /// Emit one liquid particle into a free slot. It joins the solve at the
    /// next step. Emit drops about one particle spacing (`2r`) apart, or they
    /// push each other apart violently.
    pub fn emit_liquid(
        &mut self,
        liquid: LiquidHandle,
        position: Vec3,
        velocity: Vec3,
    ) -> Result<u32, SimError> {
        self.start()?;
        let f = self
            .fluids
            .get_mut(liquid.index())
            .filter(|f| f.kind == Kind::Liquid)
            .ok_or(SimError::NoFreeSlots)?;
        let slot = self.p.alloc().ok_or(SimError::NoFreeSlots)?;
        let inv = 1.0 / f.mass;
        self.p.init(
            slot,
            Kind::Liquid,
            liquid.0 as u16,
            Vec3A::from(position),
            Vec3A::from(velocity),
            inv,
            0,
        );
        f.members.push(slot);
        self.fluid
            .reset_particle(slot as usize, f.params.rest_density);
        self.touch[slot as usize] = false;
        self.members_dirty = true;
        Ok(slot)
    }

    /// Particles retired during the last `step`/`advance` call.
    pub fn retired(&self) -> &[RetiredParticle] {
        &self.retired
    }

    /// Change a liquid's settings (viscosity, surface tension, …) at any time.
    pub fn set_liquid_params(&mut self, liquid: LiquidHandle, params: LiquidParams) {
        if let Some(f) = self.fluids.get_mut(liquid.index()) {
            let rest_changed = f.params.rest_density != params.rest_density;
            f.params = params;
            if rest_changed {
                f.mass = f.params.rest_density * f.particle_volume;
                for &i in &f.members {
                    self.p.inv_mass[i as usize] = 1.0 / f.mass;
                }
            }
        } else if let Some(spec) = self.liquid_specs.get_mut(liquid.index()) {
            spec.params = params;
        }
    }

    pub fn liquid_params(&self, liquid: LiquidHandle) -> Option<&LiquidParams> {
        self.fluids
            .get(liquid.index())
            .map(|f| &f.params)
            .or_else(|| self.liquid_specs.get(liquid.index()).map(|s| &s.params))
    }

    /// Live particles of a liquid.
    pub fn liquid_count(&self, liquid: LiquidHandle) -> usize {
        self.fluids
            .get(liquid.index())
            .map_or(0, |f| f.members.len())
    }

    /// Positions (and optionally velocities) of a liquid's live particles.
    pub fn liquid_particles(
        &self,
        liquid: LiquidHandle,
        positions: &mut Vec<Vec3>,
        velocities: Option<&mut Vec<Vec3>>,
    ) {
        positions.clear();
        let Some(f) = self.fluids.get(liquid.index()) else {
            return;
        };
        positions.extend(
            f.members
                .iter()
                .map(|&i| Vec3::from(self.p.pos[i as usize])),
        );
        if let Some(v) = velocities {
            v.clear();
            v.extend(
                f.members
                    .iter()
                    .map(|&i| Vec3::from(self.p.vel[i as usize])),
            );
        }
    }

    /// SPH density of each live particle of a liquid from the last solver
    /// iteration, kg/m³ (same order as [`liquid_particles`](Self::liquid_particles)).
    pub fn liquid_densities(&self, liquid: LiquidHandle, out: &mut Vec<f32>) {
        out.clear();
        if let Some(f) = self.fluids.get(liquid.index()) {
            out.extend(f.members.iter().map(|&i| self.fluid.density[i as usize]));
        }
    }

    // ------------------------------------------------------------- cloth control

    fn cloth(&self, h: ClothHandle) -> Option<&Cloth> {
        self.cloths.get(h.index())
    }

    /// Wind on a cloth, m/s. Call every frame as the wind changes.
    pub fn set_cloth_wind(&mut self, cloth: ClothHandle, wind: Vec3) {
        if let Some(c) = self.cloths.get_mut(cloth.index()) {
            c.wind = wind.into();
        } else if let Some(s) = self.cloth_specs.get_mut(cloth.index()) {
            s.wind = wind;
        }
    }

    /// 0 for stiff canvas to 1 for silk; recomputes the bending compliance.
    pub fn set_cloth_softness(&mut self, cloth: ClothHandle, softness: f32) {
        if let Some(c) = self.cloths.get_mut(cloth.index()) {
            c.set_softness(softness);
        } else if let Some(s) = self.cloth_specs.get_mut(cloth.index()) {
            s.softness = softness;
        }
    }

    /// Air drag and lift coefficients (`½ C ρ_air`, kg/m³).
    pub fn set_cloth_aero(&mut self, cloth: ClothHandle, drag: f32, lift: f32) {
        if let Some(c) = self.cloths.get_mut(cloth.index()) {
            c.drag = drag.max(0.0);
            c.lift = lift.max(0.0);
        }
    }

    /// Cloth grid `(particles per row, rows)`; particle `row · columns + column`
    /// of [`cloth_particles`](Self::cloth_particles), row 0 at the top.
    pub fn cloth_grid(&self, cloth: ClothHandle) -> Option<(u32, u32)> {
        self.cloth(cloth).map(|c| (c.columns, c.rows))
    }

    /// Cloth-local indices of the pinned particles.
    pub fn cloth_pins(&self, cloth: ClothHandle) -> &[u32] {
        self.cloth(cloth).map_or(&[], |c| &c.pins)
    }

    /// Current positions of the pinned particles (same order as [`cloth_pins`](Self::cloth_pins)).
    pub fn cloth_pin_positions(&self, cloth: ClothHandle) -> Vec<Vec3> {
        self.cloth(cloth)
            .map(|c| {
                c.pins
                    .iter()
                    .map(|&i| Vec3::from(self.p.pos[(c.start + i) as usize]))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Move pin `pin` (an index into [`cloth_pins`](Self::cloth_pins)) to
    /// `position` by the end of the next frame, interpolated across substeps.
    pub fn set_cloth_pin(&mut self, cloth: ClothHandle, pin: usize, position: Vec3) {
        if let Some(c) = self.cloths.get_mut(cloth.index()) {
            c.set_pin_target(pin, position);
        }
    }

    /// Move every pin rigidly with the cloth's carrier (a banner pole): each
    /// pin goes to `rotation · (rest − rest_centre) + translation`, where
    /// `rest_centre` is the cloth's `position` as added.
    pub fn set_cloth_pin_transform(
        &mut self,
        cloth: ClothHandle,
        translation: Vec3,
        rotation: Quat,
    ) {
        if let Some(c) = self.cloths.get_mut(cloth.index()) {
            c.set_pin_transform(translation, rotation);
        }
    }

    /// Per-particle wetness, 0–1, in grid order.
    pub fn cloth_wetness(&self, cloth: ClothHandle) -> &[f32] {
        self.cloth(cloth).map_or(&[], |c| &c.wetness)
    }

    /// Positions of a cloth's particles in grid order.
    pub fn cloth_particles(&self, cloth: ClothHandle, out: &mut Vec<Vec3>) {
        out.clear();
        if let Some(c) = self.cloth(cloth) {
            out.extend(c.range().map(|i| Vec3::from(self.p.pos[i])));
        }
    }

    /// Fill a cloth's render surface: smooth positions, normals, UVs,
    /// triangle indices and wetness per render vertex.
    pub fn fill_cloth_surface(&self, cloth: ClothHandle, out: &mut ClothSurface) {
        if let Some(c) = self.cloth(cloth) {
            fill_surface(&self.p.pos[c.range()], &c.wetness, c.columns, c.rows, out);
        }
    }

    // ---------------------------------------------------------- soft-body control

    /// 0 firm to 1 jelly; recomputes the compliance from the particle count.
    pub fn set_softbody_softness(&mut self, body: SoftbodyHandle, softness: f32) {
        if body.index() < self.soft.bodies.len() {
            self.soft.set_softness(body.index(), softness);
        } else if let Some(s) = self.soft_specs.get_mut(body.index()) {
            s.softness = softness;
        }
    }

    /// Number of leading particles on the body's surface (the liquid's boundary).
    pub fn softbody_surface_count(&self, body: SoftbodyHandle) -> usize {
        self.soft
            .bodies
            .get(body.index())
            .map_or(0, |b| b.surface_count as usize)
    }

    pub fn softbody_count(&self, body: SoftbodyHandle) -> usize {
        self.soft
            .bodies
            .get(body.index())
            .map_or(0, |b| b.count as usize)
    }

    /// Best-fit centre and rotation of a body relative to its rest shape.
    pub fn softbody_frame(&self, body: SoftbodyHandle) -> Option<(Vec3, Quat)> {
        self.soft.bodies.get(body.index()).map(|b| {
            (
                Vec3::from(b.center),
                crate::math::quat_from_m3(&b.rotation).normalize(),
            )
        })
    }

    /// Rest centre of a body (the frame [`softbody_frame`](Self::softbody_frame) is relative to).
    pub fn softbody_rest_center(&self, body: SoftbodyHandle) -> Option<Vec3> {
        self.soft.bodies.get(body.index()).map(|b| b.rest_center)
    }

    /// Positions of a body's particles.
    pub fn softbody_particles(&self, body: SoftbodyHandle, out: &mut Vec<Vec3>) {
        out.clear();
        if let Some(b) = self.soft.bodies.get(body.index()) {
            out.extend(b.range().map(|i| Vec3::from(self.p.pos[i])));
        }
    }

    /// Bind a render mesh (world space, over the body's rest shape) to a body.
    pub fn bind_softbody_mesh(
        &mut self,
        body: SoftbodyHandle,
        vertices: &[Vec3],
        normals: &[Vec3],
    ) -> Result<SkinBinding, SimError> {
        self.start()?;
        if body.index() >= self.soft.bodies.len() {
            return Err(SimError::SoftbodyTooSmall(body.index()));
        }
        Ok(self.soft.bind(body.index(), self.radius, vertices, normals))
    }

    /// Deformed world positions and normals of a bound mesh.
    pub fn skin_softbody(
        &self,
        binding: &SkinBinding,
        positions: &mut Vec<Vec3>,
        normals: &mut Vec<Vec3>,
    ) {
        self.soft.skin(&self.p, binding, positions, normals);
    }

    // ------------------------------------------------------------- smoke control

    pub fn set_smoke_source(&mut self, source: Vec3) {
        if let Some(s) = &mut self.smoke {
            s.source = source;
        } else if let Some(s) = &mut self.smoke_spec {
            s.source = Some(source);
        }
    }

    /// Tracers per second, capped at the pool's `max_rate`.
    pub fn set_smoke_rate(&mut self, rate: f32) {
        if let Some(s) = &mut self.smoke {
            s.rate = rate.max(0.0);
        } else if let Some(s) = &mut self.smoke_spec {
            s.rate = rate.max(0.0);
        }
    }

    /// Buoyancy of heated air, m/s².
    pub fn set_smoke_heat(&mut self, heat: f32) {
        if let Some(s) = &mut self.smoke {
            s.heat = heat;
        } else if let Some(s) = &mut self.smoke_spec {
            s.heat = heat;
        }
    }

    pub fn set_smoke_cooling(&mut self, cooling: f32) {
        if let Some(s) = &mut self.smoke {
            s.cooling = cooling.max(0.0);
        } else if let Some(s) = &mut self.smoke_spec {
            s.cooling = cooling.max(0.0);
        }
    }

    /// Wind and how strongly the air follows it (1/s).
    pub fn set_smoke_wind(&mut self, wind: Vec3, coupling: Option<f32>) {
        if let Some(s) = &mut self.smoke {
            s.wind = wind;
            if let Some(c) = coupling {
                s.wind_coupling = c.max(0.0);
            }
        } else if let Some(s) = &mut self.smoke_spec {
            s.wind = wind;
            if let Some(c) = coupling {
                s.wind_coupling = c.max(0.0);
            }
        }
    }

    /// Release one extra tracer; `false` if the pool is full or there is no smoke.
    pub fn emit_smoke(&mut self, position: Vec3) -> bool {
        self.smoke
            .as_mut()
            .is_some_and(|s| s.emit(Vec3A::from(position)))
    }

    pub fn smoke_tracer_count(&self) -> usize {
        self.smoke.as_ref().map_or(0, |s| s.tracers.len())
    }

    pub fn smoke_tracer_capacity(&self) -> usize {
        self.smoke.as_ref().map_or(0, |s| s.capacity)
    }

    /// Positions and normalized ages (`age / lifetime`) of the live tracers.
    pub fn smoke_tracers(&self, positions: &mut Vec<Vec3>, ages: Option<&mut Vec<f32>>) {
        positions.clear();
        let Some(s) = &self.smoke else { return };
        positions.extend(s.tracers.pos.iter().map(|&p| Vec3::from(p)));
        if let Some(a) = ages {
            a.clear();
            a.extend(s.tracers.age.iter().map(|&t| t / s.lifetime));
        }
    }

    /// Air temperature per air particle (0 ambient, 1 at the source).
    pub fn smoke_air(&self, positions: &mut Vec<Vec3>, temperatures: Option<&mut Vec<f32>>) {
        positions.clear();
        let Some(s) = &self.smoke else { return };
        positions
            .extend((s.air_start..s.air_start + s.air_count).map(|i| Vec3::from(self.p.pos[i])));
        if let Some(t) = temperatures {
            t.clear();
            t.extend_from_slice(&s.temperature);
        }
    }

    // ----------------------------------------------------------------- rendering

    /// Build the smoke density/light volume for ray marching.
    pub fn build_smoke_volume(&self, opts: &SmokeVolumeOptions, out: &mut SmokeVolume) {
        let Some(s) = &self.smoke else { return };
        let bounds = opts.bounds.unwrap_or_else(|| {
            let c = s.container;
            Aabb::new(
                c.min,
                Vec3::new(c.max.x, c.max.y - 0.1 * c.size().y, c.max.z),
            )
        });
        smoke_volume::build(&s.tracers, s.lifetime, bounds, opts, out);
    }

    /// Build a liquid's surface field for ray marching.
    pub fn build_liquid_surface(
        &self,
        liquid: LiquidHandle,
        opts: &LiquidSurfaceOptions,
        out: &mut LiquidSurfaceField,
    ) {
        let Some(f) = self.fluids.get(liquid.index()) else {
            return;
        };
        let bounds = opts
            .bounds
            .or_else(|| {
                self.config.container.map(|c| {
                    Aabb::new(
                        c.bounds.min - Vec3::new(0.03, 0.02, 0.03),
                        c.bounds.max + Vec3::new(0.03, 0.02, 0.03),
                    )
                })
            })
            .unwrap_or_else(|| {
                let b = f.members.iter().fold(Aabb::inverted(), |b, &i| {
                    b.union_point(Vec3::from(self.p.pos[i as usize]))
                });
                if b.is_empty() {
                    Aabb::from_center_half(Vec3::ZERO, Vec3::splat(0.5))
                } else {
                    b.expanded(b.size().max_element() * 0.5 + 4.0 * self.radius)
                }
            });
        let solids: Vec<u32> = self
            .active
            .iter()
            .copied()
            .filter(|&i| matches!(self.p.kind[i as usize], Kind::Cloth | Kind::Softbody))
            .collect();
        let input = SurfaceInputs {
            positions: &self.p.pos,
            velocities: &self.p.vel,
            liquid: &f.members,
            solids: &solids,
            radius: self.radius,
            spacing: 2.0 * self.radius,
            colliders: &self.colliders,
        };
        liquid_surface::build(&input, bounds, opts, out);
    }

    // ------------------------------------------------------------------ queries

    pub fn is_started(&self) -> bool {
        self.started
    }

    /// Radius of every particle, m (0 before start with a budget).
    pub fn particle_radius(&self) -> f32 {
        self.radius
    }

    /// Particle slots (live plus free).
    pub fn capacity(&self) -> usize {
        self.p.capacity()
    }

    /// Live particles.
    pub fn particle_count(&self) -> usize {
        self.active.len()
    }

    /// Free slots available to [`emit_liquid`](Self::emit_liquid).
    pub fn free_slots(&self) -> usize {
        self.p.free_count()
    }

    pub fn substeps(&self) -> u32 {
        self.substeps
    }

    /// Change the substep count (takes effect on the next step).
    pub fn set_substeps(&mut self, substeps: u32) {
        self.substeps = substeps.max(1);
        self.config.substeps = Some(self.substeps);
    }

    pub fn gravity(&self) -> Vec3 {
        self.gravity
    }

    pub fn set_gravity(&mut self, gravity: Vec3) {
        self.gravity = gravity;
    }

    /// Simulated time, s.
    pub fn time(&self) -> f64 {
        self.time
    }

    /// Some fluid particle had more than 64 neighbors at the last substep.
    pub fn neighbor_overflow(&self) -> bool {
        self.fluid.nbrs.overflow
    }

    /// Some particle had more than 32 contacts at the last substep.
    pub fn contact_overflow(&self) -> bool {
        self.contacts.as_ref().is_some_and(|c| c.overflow)
    }

    /// Visit every live particle: `(slot, kind, position, velocity)`.
    pub fn for_each_particle(&self, mut f: impl FnMut(u32, Kind, Vec3, Vec3)) {
        for &i in &self.active {
            let iu = i as usize;
            f(
                i,
                self.p.kind[iu],
                Vec3::from(self.p.pos[iu]),
                Vec3::from(self.p.vel[iu]),
            );
        }
    }
}

/// Grid points `spacing` apart filling `b`, offset half a spacing from its corner (`fillBox`).
fn fill_box(b: &Aabb, spacing: f32) -> Vec<Vec3> {
    let mut out = Vec::new();
    if b.is_empty() {
        return out;
    }
    let mut y = b.min.y + spacing / 2.0;
    while y < b.max.y {
        let mut z = b.min.z + spacing / 2.0;
        while z < b.max.z {
            let mut x = b.min.x + spacing / 2.0;
            while x < b.max.x {
                out.push(Vec3::new(x, y, z));
                x += spacing;
            }
            z += spacing;
        }
        y += spacing;
    }
    out
}

/// A hashed point set for "is any point within d" queries during layout (`clearOf`).
struct PointGrid {
    cell: f32,
    map: std::collections::HashMap<(i32, i32, i32), Vec<Vec3>>,
}

impl PointGrid {
    fn new(points: &[Vec3], cell: f32) -> Self {
        let mut map: std::collections::HashMap<(i32, i32, i32), Vec<Vec3>> =
            std::collections::HashMap::new();
        for &p in points {
            map.entry(Self::key(p, cell)).or_default().push(p);
        }
        Self { cell, map }
    }

    fn key(p: Vec3, cell: f32) -> (i32, i32, i32) {
        (
            (p.x / cell).floor() as i32,
            (p.y / cell).floor() as i32,
            (p.z / cell).floor() as i32,
        )
    }

    fn any_within(&self, p: Vec3, d: f32) -> bool {
        let (cx, cy, cz) = Self::key(p, self.cell);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(list) = self.map.get(&(cx + dx, cy + dy, cz + dz))
                        && list.iter().any(|q| (*q - p).length_squared() < d * d)
                    {
                        return true;
                    }
                }
            }
        }
        false
    }
}

// The host steps simulations on worker threads.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<Simulation>();
};

/// Per-phase wall-clock accumulators, printed on drop with the `timing` feature.
#[cfg(feature = "timing")]
mod timing {
    use std::sync::Mutex;
    use std::time::Instant;
    pub static TOTALS: Mutex<Vec<(&'static str, f64, u64)>> = Mutex::new(Vec::new());
    pub struct Scope(&'static str, Instant);
    impl Scope {
        pub fn new(name: &'static str) -> Self {
            Scope(name, Instant::now())
        }
    }
    impl Drop for Scope {
        fn drop(&mut self) {
            let ms = self.1.elapsed().as_secs_f64() * 1000.0;
            let mut t = TOTALS.lock().unwrap();
            match t.iter_mut().find(|e| e.0 == self.0) {
                Some(e) => {
                    e.1 += ms;
                    e.2 += 1;
                }
                None => t.push((self.0, ms, 1)),
            }
        }
    }
    pub fn report() {
        let t = TOTALS.lock().unwrap();
        for (name, ms, n) in t.iter() {
            eprintln!(
                "{name:<24} {ms:10.2} ms total {:8.4} ms/call {n:8} calls",
                ms / *n as f64
            );
        }
    }
}

#[cfg(feature = "timing")]
impl Drop for Simulation {
    fn drop(&mut self) {
        let rows = &self.fluid.rows;
        if !rows.is_empty() {
            let total: usize = rows
                .iter()
                .map(|&i| self.fluid.nbrs.row(i as usize).len())
                .sum();
            eprintln!(
                "fluid rows {}, mean neighbors {:.1}",
                rows.len(),
                total as f64 / rows.len() as f64
            );
        }
        timing::report();
    }
}
