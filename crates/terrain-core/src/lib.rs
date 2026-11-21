//! The headless terrain stack: description, generation, storage, lighting,
//! meshing, and save formats.
//!
//! This crate has no window, GPU, audio, or renderer dependency. The engine
//! crate re-exports these modules, and the `terrain-gen` acceptance binary
//! links only this crate so the crate-graph check can enforce that boundary.
//!
//! The module declarations stay public so `engine::terrain::<module>` keeps
//! resolving, but the generation internals are crate-private. This example must
//! stay a compile error from outside the crate:
//!
//! ```compile_fail
//! // `noise` is a public module, but batch 1 made its items crate-private.
//! use terrain_core::noise::unit;
//! # fn main() {}
//! ```
//!
//! ```compile_fail
//! // Batch 2 narrowed the batch-2 modules' internals; `Budget` is crate-private
//! // even though `residency` itself stays a public module.
//! use terrain_core::residency::Budget;
//! # fn main() {}
//! ```

pub mod climate;
#[allow(dead_code)]
pub mod content;
pub mod curve;
#[allow(dead_code)]
pub mod description;
#[allow(dead_code)]
pub mod editlog;
pub mod error;
// Batch 1 of the visibility narrowing made these modules crate-internal. Their
// unreachable definitions stay in the tree for the generation stages and the
// unit tests, so they opt out of `dead_code` rather than being deleted by a
// visibility-only change.
#[allow(dead_code)]
pub mod features;
#[allow(dead_code)]
pub mod graph;
pub mod hash;
#[allow(dead_code)]
pub mod header;
pub mod height;
#[allow(dead_code)]
pub mod hydro;
pub mod lattice;
#[allow(dead_code)]
pub mod light2;
pub mod lod;
#[allow(dead_code)]
pub mod mesher2;
#[allow(dead_code)]
pub mod metrics2;
#[allow(dead_code)]
pub mod noise;
pub mod padded;
pub mod pipeline;
#[allow(dead_code)]
pub mod residency;
#[allow(dead_code)]
pub mod revisions;
#[allow(dead_code)]
pub mod save;
#[allow(dead_code)]
pub mod section;
#[allow(dead_code)]
pub mod strata;
#[allow(dead_code)]
pub mod surface;
#[allow(dead_code)]
pub mod volume;
#[allow(dead_code)]
pub mod world_save;
