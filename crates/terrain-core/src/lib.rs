//! The headless terrain stack: description, generation, storage, lighting,
//! meshing, and save formats.
//!
//! This crate has no window, GPU, audio, or renderer dependency. The engine
//! crate re-exports these modules, and the `terrain-gen` acceptance binary
//! links only this crate so the crate-graph check can enforce that boundary.

pub mod climate;
pub mod content;
pub mod curve;
pub mod description;
pub mod editlog;
pub mod error;
pub mod features;
pub mod graph;
pub mod hash;
pub mod header;
pub mod height;
pub mod hydro;
pub mod lattice;
pub mod light2;
pub mod lod;
pub mod mesher2;
pub mod metrics2;
pub mod noise;
pub mod padded;
pub mod pipeline;
pub mod residency;
pub mod revisions;
pub mod save;
pub mod section;
pub mod strata;
pub mod surface;
pub mod volume;
pub mod world_save;
