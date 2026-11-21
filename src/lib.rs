//! Valescent Engine: a voxel engine written in Rust.
//!
//! The binary in `src/main.rs` is a thin shell around this crate. Everything
//! the engine does lives here so it can be tested, benchmarked, and reused
//! without a window.

mod app;
mod block;
mod camera;
mod config;
mod demo;
mod frustum;
mod headless;
mod metrics;
mod overlay;
mod player;
mod raycast;
mod renderer;
mod sky;
mod startup;
mod v0;

pub use app::RunError;
pub use renderer::RendererError;
pub use startup::run;
pub use v0::{generation, light, mesh, mesher, streaming, voxel, world};

pub use terrain_core::{
    climate, content, curve, description, editlog, error, features, graph, hash, header, height,
    hydro, lattice, light2, lod, mesher2, metrics2, noise, padded, pipeline, residency, revisions,
    save, section, strata, surface, volume, world_save,
};

pub use world::{ChunkCoord, SaveError};
