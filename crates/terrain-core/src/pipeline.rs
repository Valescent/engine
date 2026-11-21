//! The column pipeline, in the fixed `F1.5` order.
//!
//! `HeightTile` -> fill -> detail -> carve -> spans -> features. The macro fields
//! are graph data evaluated on the 4-block lattice; everything per block happens
//! here, after interpolation, exactly once per column.
//!
//! Order that must not change:
//!
//! 1. macro height from the lattice, bilinear to blocks,
//! 2. per-block detail, then terrace, then river carve, one `top_solid_y`,
//! 3. solid fill from the surface stack, strata resolved symbolically,
//! 4. 3D detail and cave carve (`crate::volume::Volume::apply`),
//! 5. water fill, which never floods a cave (`D7.1`).
//!
//! Water is placed after the volume pass rather than at step 2 as `F1.5` words it.
//! The result is identical because water only occupies `top+1..=level`, which the
//! volume pass never writes; the pass needs a solid/air-only column to tell rock
//! from water.
//!
//! The world graph here is the v1 authoring of `C1.1`/`C1.6`/`C2.2`. Curve knots
//! live in [`Curves::v1`] and each one is a column of `terrain-interview.md`.
//!
//! The implementation is split into [`terrain`] (generator, caches, memo),
//! [`stages`] (column and section assembly, materials, biomes), and [`hydro`]
//! (world hydrology handle). Every public item is re-exported here, so
//! `pipeline::<item>` paths are unchanged.

// Coordinate arithmetic only: every cast below is between in-range world
// coordinates and their index or `f32` representation. World extents are far
// inside `f32`'s 24-bit mantissa and every index is non-negative.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

mod hydro;
mod stages;
mod terrain;

#[cfg(test)]
mod tests;

pub use hydro::HydroTileRef;
pub use stages::{AIR, TerrainMaterials, WATER};
pub use terrain::{Curves, Terrain, TileData};
