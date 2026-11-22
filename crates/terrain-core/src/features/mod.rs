//! W8 - feature discovery and application: ores, trees, scatter.
//!
//! Two phases (`H1`, `H2`). `discover` is a pure function of a seed, one
//! `32³` region, and the spec list; it returns candidates sorted by id.
//! `gather` merges the `3 x 3 x 3` neighbourhood around one region. `apply`
//! writes the gathered candidates into one section's block array through their
//! declared `Mask`, in the fixed `WRITE_ORDER`.
//!
//! # Region ownership
//!
//! A region is `REGION_SIZE` blocks on every axis, aligned to the section grid
//! (`REGION_SIZE == SECTION_SIZE`). A candidate's centre always lies inside the
//! region that owns it, so a `3 x 3 x 3` gather has no duplicate centre. A
//! candidate's blocks may spill up to `MAX_EXTENT <= 32` in the direction of a
//! neighbour, which is why application gathers exactly that neighbourhood.
//!
//! # Lowest id wins
//!
//! `gather` sorts by id and `apply` writes in id order. Two features that
//! claim one block therefore resolve to the lower id: the first writer wins and
//! the second is refused by its own mask. Ids never depend on iteration order.
//!
//! # Masks
//!
//! Nothing writes water or bedrock, ever. Beyond that a feature may only write
//! where its `Mask` allows, and `validate_spec` rejects a spec that mixes a
//! solid-replacing mask with an air-only one (`H2.3`).
//!
//! # Determinism
//!
//! Integer hash arithmetic only: no `HashMap`, no `sin`/`cos`/`pow`, no NaN, and
//! every loop has a fixed upper bound. Applying the same candidates twice is
//! idempotent (`H2.4`).

pub mod ore;
pub mod scatter;
pub mod tree;

mod apply;
mod discover;
mod masks;
#[cfg(test)]
mod tests;

#[allow(unused_imports)] // re-exported for the internal stages and unit tests
pub(crate) use apply::{
    ApplyContext, ApplyReport, ApplyStats, WRITE_ORDER, WRITE_PASSES, apply, apply_counted,
    apply_section, apply_section_counted, stage_for,
};
#[allow(unused_imports)] // re-exported for the internal stages and unit tests
pub(crate) use discover::{
    Candidate, FeatureKind, FeatureSpec, GATHER_RADIUS, GATHER_REGIONS, REGION_SIZE,
    candidates_reach, discover, gather, gather_origins, region_contains, region_of, region_origin,
    region_slot, uniform_writable, v1_specs, validate_masks, validate_spec,
};
pub(crate) use masks::{
    COLUMN_BANK, COLUMN_RIVER, COLUMNS, ColumnField, FOOTPRINT, Mask, OreBlock, Palette,
};

pub(crate) use apply::Sink;
#[cfg(test)]
pub(crate) use discover::variant_with_spec;
pub(crate) use discover::{variant_spec, within};
