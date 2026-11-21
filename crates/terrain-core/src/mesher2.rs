//! `W6` - the light-aware greedy mesher over packed `u64` quad records.
//!
//! One [`Section`] becomes a [`SectionMesh`]: three independent ranges of
//! [`Quad`] records (`opaque`, `cutout`, `water`) in one per-section buffer
//! model (`M2.1`). Nothing here allocates a GPU buffer, knows a chunk, or
//! streams: the mesher reads one padded scratch copy and returns records.
//!
//! # Record layout (`M3.1`)
//!
//! Every quad is one `u64`, least-significant field first:
//!
//! ```text
//! bits  0.. 5  x       block origin, 0..=31
//! bits  5..10  y
//! bits 10..15  z
//! bits 15..20  w1      width - 1 along the face tangent `u`, so 32 is encodable
//! bits 20..25  h1      height - 1 along the face tangent `v`
//! bits 25..28  normal  0..=5 for the six faces, 6 and 7 for cross quad diagonals
//! bits 28..36  ao      four 2-bit corner values, corner 0 in the low bits
//! bits 36..40  sky     sky nibble of the air-side cell
//! bits 40..44  blk     block nibble of the air-side cell
//! bits 44..60  tile    texture-array layer
//! bits 60..64  tint    biome tint index
//! ```
//!
//! The record is exactly 64 bits, so the encoder masks nothing away when every
//! field is at its maximum.
//!
//! # Merge rules (`M1.1`, `L4.1`)
//!
//! - Opaque faces merge on `(block id, face direction, sky, block, packed AO)`.
//! - Water merges on `(face direction, sky, block)` with no AO (`M2.2`).
//! - LOD merges on `(block id, sky)`: AO and block light are zero at coarse
//!   levels, and the per-face sky approximation breaks a merge (`N3.5`).
//! - Cutout blocks are never greedily merged (`M2.4`). Cross quads emit two
//!   records with normal `6` and `7`.
//!
//! Only opaque full cubes occlude AO and cull a facing quad. Water emits only
//! against air: water/water and water/solid emit nothing (`M2.2`).
//!
//! # Deterministic order (`M1.4`)
//!
//! Ranges are separate and never interleave. Within a range the six face
//! directions appear in the fixed order `+X, -X, +Y, -Y, +Z, -Z`, then records
//! are sorted by `(y, z, x)`; cross quads follow the six face groups, sorted by
//! `(y, z, x, normal)`.
//!
//! # Skirts (`N3.2`)
//!
//! [`add_skirts`] appends a downward wall along selected section edges, into the
//! opaque range, so a coarse section's lower-resolution boundary hides the gap
//! under it instead of stitching to its finer neighbours. [`add_step_skirts`]
//! is the drop-aware variant: it reads the coarse padded scratch and fills only
//! the vertical step to the adjacent column, so a flush edge appends nothing.
//! Skirt records copy the boundary face's `tile` and `tint`, take its air-side
//! `sky`/`blk`, and force fully open AO. [`SectionMesh::skirts`] records the
//! sides already done, so a skirt set is never appended twice.
//!
//! # Padded inputs (`I2.3`, `I2.4`)
//!
//! [`PaddedBlocks`] and [`PaddedLight`] come from [`crate::padded`], the one
//! engine-wide `34³` scratch. The mesher reads the section plus all 26
//! neighbours, so corner AO at a section corner sees the diagonal sections
//! instead of defaulting them to air. [`mesh_from_neighbours`] builds that
//! scratch from the 26-section ownership array; [`mesh_from_face_neighbours`]
//! keeps the old six-face call shape and pads the edge and corner slots with
//! air.
//!
//! [`Section`]: crate::section::Section
//! [`PaddedBlocks`]: crate::padded::PaddedBlocks
//! [`PaddedLight`]: crate::padded::PaddedLight

mod cross;
mod greedy;
mod lod;
mod material;
mod record;
mod skirt;
#[cfg(test)]
mod tests;

/// Cells in one section face plane.
const PLANE_WIDTH: usize = crate::description::SECTION_SIZE as usize;

/// Cells in one section face plane.
const PLANE_CELLS: usize = PLANE_WIDTH * PLANE_WIDTH;

pub use greedy::{mesh, mesh_from_face_neighbours, mesh_from_neighbours};
pub use lod::{MeshLod, SHADE_PER_CELL, WATER_SKY_ATTEN, mesh_coarse};
pub use material::{Material, MaterialProps};
pub use record::{
    NORMAL_CROSS_A, NORMAL_CROSS_B, NORMAL_NEG_X, NORMAL_NEG_Y, NORMAL_NEG_Z, NORMAL_POS_X,
    NORMAL_POS_Y, NORMAL_POS_Z, Quad, SectionMesh, quad_count,
};
pub use skirt::{Side, SkirtEdges, add_skirts, add_step_skirts, skirt_blocks};
