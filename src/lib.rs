//! Valescent Engine: a voxel engine written in Rust.
//!
//! The binary in `src/main.rs` is a thin shell around this crate. Everything
//! the engine does lives here so it can be tested, benchmarked, and reused
//! without a window.

mod app;
#[cfg(test)]
mod bench;
mod block;
mod camera;
mod config;
mod demo;
mod frustum;
mod headless;
mod hierarchy;
mod horizon;
mod metrics;
mod overlay;
mod player;
mod pool;
mod quad;
mod quad_renderer;
mod raycast;
mod renderer;
mod section;
mod sky;
mod startup;
mod v0;
mod world_io;

pub use app::RunError;
pub use renderer::RendererError;
pub use startup::run;
pub use v0::{generation, light, mesh, mesher, streaming, voxel, world};

// Internal re-exports: existing `crate::pipeline`, `crate::section`, and the
// other flat paths keep working inside the crate. The public path is
// `engine::terrain::<module>`.
//
// Keep the full module list even when a module has no current `crate::` user,
// so internal paths stay stable as code moves between modules.
#[allow(unused_imports)]
pub(crate) use terrain_core::{
    climate, content, curve, description, editlog, error, features, graph, hash, header, height,
    hydro, lattice, light2, lod, mesher2, metrics2, noise, padded, pipeline, residency, revisions,
    save, section as terrain_section, strata, surface, volume, world_save,
};

/// The supported entry point for the headless terrain stack.
///
/// `engine` depends on the headless `terrain-core` crate and exposes its
/// modules here. Use `engine::terrain::pipeline::Terrain`,
/// `engine::terrain::section::Section`, `engine::terrain::light2`, and the
/// other modules under this facade.
///
/// The flat paths such as `engine::pipeline` and `engine::section` are
/// crate-internal re-exports. They are not public API and do not resolve
/// outside `engine`.
///
/// ```
/// use engine::terrain::pipeline::Terrain;
///
/// let _new: fn(u64) -> Result<Terrain, engine::terrain::error::TerrainError> = Terrain::new;
/// ```
///
/// The flat paths are deliberately not reachable from outside the crate:
///
/// ```compile_fail
/// use engine::pipeline::Terrain;
/// ```
///
/// ```compile_fail
/// use engine::section::Section;
/// ```
pub mod terrain {
    pub use terrain_core::{
        climate, content, curve, description, editlog, error, features, graph, hash, header,
        height, hydro, lattice, light2, lod, mesher2, metrics2, noise, padded, pipeline, residency,
        revisions, save, section, strata, surface, volume, world_save,
    };
}

pub use world::{ChunkCoord, SaveError};
