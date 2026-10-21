//! Procedural tree generator: an exact Rust port of
//! [ez-tree](https://github.com/dgreenheck/ez-tree) by Daniel Greenheck
//! (MIT License; see `LICENSE-ez-tree` in this crate).
//!
//! For the same [`TreeOptions`] the port reproduces ez-tree's geometry —
//! vertex positions, normals, UVs and indices — to float precision: the RNG
//! emulates JavaScript's int32/uint32 semantics, all generation math runs in
//! `f64` using the same formulas and operation order as three.js
//! (`Quaternion`, `Euler`, `Vector3`), and the tuning quirks of the original
//! are kept so the bundled presets look the same. The integration tests
//! compare against reference data produced by running the original
//! `tree.js` under Node (see `tools/gen_fixtures.mjs`).
//!
//! Generation has two stages, like ez-tree:
//!
//! 1. [`grow`] builds a [`Skeleton`] (branch rings and leaf placements). All
//!    randomness is consumed here.
//! 2. [`mesh`] turns a skeleton into vertex buffers at a [`Detail`] level
//!    (LOD), without randomness, so several LODs can share one skeleton.
//!
//! ```
//! use ez_tree::{Detail, TreeOptions};
//!
//! let opts = TreeOptions::preset("oak_medium").unwrap();
//! let skeleton = ez_tree::grow(&opts);
//! let lods: Vec<_> = (0..Detail::LOD_COUNT)
//!     .map(|lod| ez_tree::mesh(&opts, &skeleton, &Detail::lod(lod)))
//!     .collect();
//! assert!(lods[2].triangle_count() < lods[0].triangle_count());
//! ```
//!
//! Differences from ez-tree:
//! - Bark UV `v` has `1 / bark.textureScale.y` baked in (ez-tree applies it
//!   at runtime via `texture.repeat.y`).
//! - Indices are `u32` (ez-tree stores them in a `Uint16Array`, which wraps
//!   for meshes with more than 65 536 vertices).
//! - Each vertex carries an extra wind [`Buffers::sway`] weight, and bark
//!   vertices a [`Buffers::bark`] frame (around / along the branch) for
//!   physically scaled bark mapping.
//! - Trellis growth forces are not ported; `branch.levels` is capped at 3.
//! - No materials or textures: the host maps `bark.kind` / `leaves.kind`
//!   and tints to its own assets.

mod math;
mod mesh;
mod options;
mod rng;
mod skeleton;

pub use math::{Euler, Quat};
pub use mesh::{Buffers, Detail, TreeMesh, mesh};
pub use options::{
    BarkOptions, Billboard, BranchOptions, ForceOptions, LeafOptions, PRESET_NAMES, TreeOptions,
    TreeType, XY, XYZ,
};
pub use rng::Rng;
pub use skeleton::{Section, Skeleton, SkeletonBranch, SkeletonLeaf, grow};

/// Grows and meshes a tree in one go: `mesh(opts, &grow(opts), detail)`.
pub fn generate(opts: &TreeOptions, detail: &Detail) -> TreeMesh {
    mesh(opts, &grow(opts), detail)
}
