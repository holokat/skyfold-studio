//! Character import, automatic humanoid rigging and animation retargeting for Skyfold Studio.
//!
//! Everything runs locally on the CPU: a model is read into [`model::Model`] (glTF / GLB, FBX,
//! OBJ), fitted with the standard humanoid skeleton ([`humanoid`]) from a few body markers,
//! skinned by geodesic voxel binding, and any clip on any mapped skeleton is retargeted onto it.

pub mod fit;
pub mod humanoid;
pub mod markers;
pub mod glb;
pub mod import;
pub mod model;
pub mod retarget;
pub mod rig;
pub mod skin;

pub use model::*;
