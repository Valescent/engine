//! The versioned world description: the only identity a terrain world has.
//!
//! `A1.1`, `A1.3`, `T1.2`. The description is inline, self-contained, and
//! fully hashed. Seed, platform tag, and generator version live in the world
//! header instead, because one description serves many worlds.
//!
//! Runtime settings (`VIEW_RADIUS`, LOD radii, memory caps, worker counts, fog)
//! are deliberately absent: they are not identity.
//!
//! Canonical form is struct field order, little-endian fixed-width integers,
//! `f32` as bit patterns, and length-prefixed strings. `-0.0` normalises to
//! `+0.0`; NaN and infinity are rejected rather than encoded.
//!
//! # Structure
//!
//! `schema` holds the data model, schema constants, and validation; `parse` the
//! TOML reader and emitter; `hash` the canonical byte encoding. Every item is
//! re-exported here, so `description::<item>` paths are unchanged.

mod hash;
mod parse;
mod schema;
#[cfg(test)]
mod tests;

pub use parse::load;
#[allow(unused_imports)] // crate-private re-exports keep the flat module paths
pub(crate) use parse::{from_toml_str, parse};
pub use schema::{
    Axis, Description, EmbeddedRef, HYDRO_TILE, KEYS, KeySpec, MACRO_CELL, MAX_TERRAIN_Y, SEA,
    SECTION_SIZE, SECTION_VOLUME, SECTIONS_PER_COLUMN, TILE_BLOCKS, WORLD_HEIGHT, default_v1,
};
#[allow(unused_imports)] // crate-private re-exports keep the flat module paths
pub(crate) use schema::{
    CAVE_MIN_ROOF, DEEP_Y, DESCRIPTION_FILE, DETAIL_DOWN, DETAIL_UP, FOG_END, HYDRO_BORDER,
    LOD0_RADIUS_BLOCKS, LOD1_RADIUS_BLOCKS, LOD2_RADIUS_BLOCKS, RIVER_MIN_CELLS, RIVER_RAMP_CELLS,
    SCHEMA_VERSION, W_SCALE, WorldHeader,
};
