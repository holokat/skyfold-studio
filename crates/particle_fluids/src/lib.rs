//! CPU, multi-threaded particle physics for liquids, cloth, soft bodies and
//! smoke: a Rust port of [threejs-particle-fluids] by Daniel Greenheck (MIT,
//! see `LICENSE-threejs-particle-fluids`), extended for an outdoor world editor.
//!
//! The solver is extended position-based dynamics (XPBD) with substeps, as in
//! the reference: every 1/60 s step is split into substeps; each substep
//! predicts positions, rebuilds a spatial hash, finds particle contacts, then
//! iterates the material constraints (position based fluids, shape matching,
//! cloth distance/bending/tethers) and the contact and collider corrections,
//! derives velocities, and finishes with vorticity, viscosity and friction.
//!
//! ```no_run
//! use particle_fluids::*;
//! use glam::Vec3;
//!
//! let container = Container::tank(Aabb::new(Vec3::new(-0.5, 0.0, -0.3), Vec3::new(0.5, 0.8, 0.3)));
//! let mut sim = Simulation::new(SimConfig::new(Some(container), ParticleSizing::Budget(5000)));
//! let water = sim
//!     .add_liquid(LiquidOptions {
//!         fill: LiquidFill::Box(Aabb::new(Vec3::new(-0.5, 0.0, -0.3), Vec3::new(-0.1, 0.5, 0.3))),
//!         ..Default::default()
//!     })
//!     .unwrap();
//! // Every frame, on any thread:
//! sim.step(1.0 / 60.0).unwrap();
//! let mut positions = Vec::new();
//! sim.liquid_particles(water, &mut positions, None);
//! ```
//!
//! # Module map
//!
//! | Module | Port of | Contents |
//! | --- | --- | --- |
//! | [`sim`] | `core/loop.ts`, `simulation/Simulation.ts` | [`Simulation`], configuration, handles, layout, the substep loop, runtime control |
//! | [`stepper`] | `core/frameStepper.ts` | fixed 1/60 s accumulator, ≤ 4 steps per call |
//! | `particles` | `core/particles.ts` | fixed-capacity structure-of-arrays storage with free slots |
//! | `grid` | `core/hashGrid/` | counting-sorted cell grid (dense table, Morton hash for sparse scenes); 64-neighbor lists |
//! | `contact` | `core/contact/` | particle contacts with static and kinetic friction |
//! | [`colliders`] | `core/collision/`, `sdf/` | planes, spheres, boxes, capsules (swept when moved), baked SDF meshes, and a terrain [`Heightfield`] |
//! | [`fluid`] | `fluids/` | position based fluids, boundaries, surface tension, adhesion, vorticity, XSPH, implicit viscosity |
//! | [`cloth`] | `cloth/` | rectangle graphs, distance/bending/tether constraints, aero, pins, wetness, render surface |
//! | [`softbody`] | `softbody/` | voxelizer, local/global shape matching, dual-quaternion skinning |
//! | [`gas`] | `gas/` | air, heat, buoyancy, tracers, wind |
//! | [`render`] | `gas/render/GasVolumeRenderer.ts`, `fluids/render/SurfaceField.ts` | smoke density/light volume and liquid surface field for ray marching |
//!
//! # Determinism
//!
//! Results are bit-identical between runs with the same inputs, at any thread
//! count: every parallel pass gathers into its own outputs in a fixed order,
//! reductions sum fixed-size chunks in order, and the smoke emitter uses a
//! seeded generator ([`SimConfig::seed`]).
//!
//! # Performance
//!
//! Hot passes run on rayon (a dedicated pool when [`SimConfig::threads`] is
//! set, else the global one). Serial cost on Apple Silicon is about 1.1 µs
//! per liquid particle per substep (5 000 particles at 3 substeps ≈ 16 ms of
//! CPU per step), so real-time budgets rely on several cores. When many
//! simulations share a machine, give each `threads: Some(2..4)`: small passes
//! scale poorly past that, and results do not depend on the thread count.
//! `cargo run -p particle_fluids --release --example bench` measures it.
//!
//! [threejs-particle-fluids]: https://github.com/dgreenheck/threejs-particle-fluids

pub mod cloth;
pub mod colliders;
pub(crate) mod contact;
pub mod fluid;
pub mod gas;
pub(crate) mod grid;
pub mod math;
pub mod mesh;
pub(crate) mod par;
pub(crate) mod particles;
pub mod render;
pub mod sim;
pub mod softbody;
pub mod stepper;

pub use cloth::surface::ClothSurface;
pub use cloth::{ClothOptions, ClothPins, cloth_bend_compliance};
pub use colliders::heightfield::Heightfield;
pub use colliders::primitives::{ColliderOptions, Shape};
pub use colliders::sdf::{SdfData, SdfError, bake_mesh_to_sdf};
pub use fluid::LiquidParams;
pub use gas::SmokeOptions;
pub use glam;
pub use grid::MAX_NEIGHBORS;
pub use math::Aabb;
pub use mesh::TriangleMesh;
pub use particles::Kind;
pub use render::liquid_surface::{FIELD_BAND, LiquidSurfaceField, LiquidSurfaceOptions};
pub use render::smoke_volume::{SmokeVolume, SmokeVolumeOptions};
pub use sim::{
    ClothHandle, ColliderHandle, Container, FIXED_DT, LiquidFill, LiquidHandle, LiquidOptions,
    ParticleSizing, RetireReason, RetiredParticle, SimConfig, SimError, Simulation, SmokeHandle,
    SoftbodyHandle,
};
pub use softbody::skin::SkinBinding;
pub use softbody::voxelize::{VoxelShape, VoxelizeResult, voxelize};
pub use softbody::{ShapeMatching, SoftbodyOptions, softbody_compliance};
pub use stepper::{FrameStepper, StepInfo};
