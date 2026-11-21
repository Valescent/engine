//! W4 - surface rule precedence and the column stack.
//!
//! `E4.1`-`E4.5`. One ordered rule list per biome, one shared evaluator, first
//! match wins per column, and a complete depth-labelled stack per column.
//!
//! # The nine slots
//!
//! Precedence is fixed (`E4.5`) and this module owns slots 1-5 and 8-9; the
//! biome's rule list contributes slots 6-7:
//!
//! 1. river or lake bed
//! 2. underwater sea floor (`top_solid_y < SEA`)
//! 3. beach band (`0 <= base <= +3` and `top >= SEA`)
//! 4. snow cap
//! 5. slope stone
//! 6. biome topsoil
//! 7. biome filler
//! 8. stone
//! 9. bedrock at `y = 0`
//!
//! Slots 6 and 7 are the first and the later matches of the biome list: the
//! first rule is the topsoil, every later rule is a filler that may stack
//! (`E4.2`). A biome list is mandatory, so "no rule matched" is a table bug;
//! [`SurfaceTable::validate_table`] rejects a missing catch-all.
//!
//! Bedrock is the absolute floor as well as the last fallback (`C4.1`): a column
//! whose top is `y = 0` is bedrock, never sea floor, so slot 9 wins there even
//! though its number is last.
//!
//! # Symbolic stone
//!
//! Slot 8 stores [`Layer::Stone`], not an id (`G1.2`). The caller resolves it
//! through [`Strata::stone_at`](crate::strata::Strata::stone_at) (or
//! [`Strata::stone_in`](crate::strata::Strata::stone_in) with the cached column
//! this module hands back), so deep rock, strata bands, and the dither band stay
//! one resolver.
//!
//! # Determinism
//!
//! Predicates compare integers and `f32` and nothing else. The slope dither and
//! the snow jitter are keyed on `(x, z)` only, never on `y`, so a column has one
//! material whatever order chunks are generated in (`E3.3`). No `sin`, `cos`,
//! `pow`, `exp`, `ln`, no `mul_add`, no hash map, and no float reduction.
//!
//! The implementation is split into `rules` (the predicate vocabulary, the ordered
//! rule table, the evaluator, and column resolution) and `seafloor` (the locked
//! seabed, bed, and beach materials and their band tables). Every public item is
//! re-exported here, so `surface::<item>` paths are unchanged.

mod rules;
mod seafloor;

#[cfg(test)]
mod tests;

pub use rules::{
    BiomeRules, ColumnFacts, DITHER_SHARE, Layer, MAX_BIOMES, Order, Predicate, Rule, SLOTS,
    STONE_DROP, SurfaceStack, SurfaceTable, T_SNOW, snow_jitter,
};
pub use seafloor::{
    SEABED_CLAY_SHARE, SEABED_DITHER_SHARE, SEABED_SHELF_LAST_DEPTH, SEABED_SHORE_LAST_DEPTH,
    SEABED_SLOPE_LAST_DEPTH, SeabedMaterials, T_COLD_SHELF, T_COLD_SHORE,
};
