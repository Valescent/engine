//! The locked-slot seabed, bed, and beach materials (`E4.7`, `D3.3`, `C1.6`).
//!
//! Band selection is code because the depth windows are pinned; the block ids
//! behind every band are content data on the `SurfaceTable`. These materials fill
//! slots 1-3 before the biome list is consulted, so a biome topsoil never reaches
//! a bed, a sea floor, or a beach (`E4.5`).

use crate::description::SEA;
use crate::hash::{hash01, hash2};

use super::rules::{ColumnFacts, Layer, Predicate, SurfaceTable, T_SNOW, holds};

// Sea-floor bands (`E4.7`). Depth is always `d = SEA - top_solid_y`, an offset
// from sea level, never an absolute `y`.
/// Last depth of the shore band: `1..=4`.
pub const SEABED_SHORE_LAST_DEPTH: i32 = 4;

/// Last depth of the shelf band: `5..=14`.
pub const SEABED_SHELF_LAST_DEPTH: i32 = 14;

/// Last depth of the slope band: `15..=24`; `>= 25` is the abyss.
pub const SEABED_SLOPE_LAST_DEPTH: i32 = 24;

/// Share of columns at a band edge that take the next, deeper material.
pub const SEABED_DITHER_SHARE: f32 = 0.35;

/// Share of warm shelf columns that take a hashed clay patch.
pub const SEABED_CLAY_SHARE: f32 = 0.15;

/// Temperature below which the shore band turns from sand to gravel.
///
/// A calibration start, not a magic number: it is the same scale as
/// [`T_SNOW`], and a colder shore is a description change.
pub const T_COLD_SHORE: f32 = 0.25;

/// Temperature below which the shelf band turns from sand to gravel.
pub const T_COLD_SHELF: f32 = 0.20;

/// Thickness of a river or lake bed layer, in blocks (`D3.3` allows 1-2).
pub(super) const BED_LAYER_DEPTH: i32 = 2;

/// Thickness of a sea-floor layer, in blocks (`E4.7`: three, then stone).
pub(super) const SEABED_LAYER_DEPTH: i32 = 3;

/// Thickness of a beach layer, in blocks.
pub(super) const BEACH_LAYER_DEPTH: i32 = 4;

/// Largest river slope that still takes the sand bed (`D3.3`).
const FLAT_RIVER_DROP: i32 = 1;

/// Domain tag of the sea-floor band dither (`E4.7`).
pub(super) const SEAFLOOR_DITHER_DOMAIN: &str = "seafloor_dither";

/// Domain tag of the sea-floor clay patches (`E4.7`).
pub(super) const SEAFLOOR_CLAY_DOMAIN: &str = "seafloor_clay";

/// The materials of the four sea-floor depth bands (`E4.7`).
///
/// The band *selection* is code because `E4.7` pins the depth windows; the
/// block ids behind every band are content data, so a pack can name
/// `core:sand`, `core:gravel`, and `core:clay` without this module knowing
/// numeric ids.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeabedMaterials {
    /// `d = 1..=4`, warm.
    pub shore_sand: Layer,
    /// `d = 1..=4`, `temp < T_COLD_SHORE`.
    pub shore_gravel: Layer,
    /// `d = 5..=14`, warm and no clay patch.
    pub shelf_sand: Layer,
    /// `d = 5..=14`, `temp < T_COLD_SHELF`.
    pub shelf_gravel: Layer,
    /// `d = 5..=14`, warm with a hashed patch.
    pub shelf_clay: Layer,
    /// `d = 15..=24`.
    pub slope_gravel: Layer,
    /// `d >= 25`.
    pub abyss_clay: Layer,
}

impl SurfaceTable {
    /// Slot 1 material: sand on a flat river bed, gravel elsewhere (`D3.3`).
    pub(super) fn bed_layer(&self, facts: &ColumnFacts) -> Layer {
        if facts.is_river && facts.slope_drop > FLAT_RIVER_DROP {
            self.river_bed_gravel
        } else {
            self.river_bed_sand
        }
    }

    /// Slot 2 material, banded by `d = SEA - top_solid_y` (`E4.7`).
    ///
    /// Ordered, first match wins: shore `1..=4`, shelf `5..=14`, slope
    /// `15..=24`, abyss `>= 25`. At the last depth of a band the next material
    /// wins on a `hash01(seed, "seafloor_dither", x, z) < 0.35` column, which
    /// makes the edge ragged without moving the band boundaries.
    pub(super) fn seabed_layer(&self, facts: &ColumnFacts) -> Layer {
        let depth = SEA.saturating_sub(facts.top);
        let dithered = self.seabed_dithers(facts);

        if holds(
            &Predicate::DepthBelowSeaIn(1, SEABED_SHORE_LAST_DEPTH),
            facts,
            0,
        ) {
            if depth == SEABED_SHORE_LAST_DEPTH && dithered {
                return self.seabed.slope_gravel;
            }
            if facts.temp < T_COLD_SHORE {
                self.seabed.shore_gravel
            } else {
                self.seabed.shore_sand
            }
        } else if holds(
            &Predicate::DepthBelowSeaIn(SEABED_SHORE_LAST_DEPTH + 1, SEABED_SHELF_LAST_DEPTH),
            facts,
            0,
        ) {
            if depth == SEABED_SHELF_LAST_DEPTH && dithered {
                return self.seabed.slope_gravel;
            }
            if facts.temp < T_COLD_SHELF {
                self.seabed.shelf_gravel
            } else if self.shelf_clay_patch(facts) {
                self.seabed.shelf_clay
            } else {
                self.seabed.shelf_sand
            }
        } else if holds(
            &Predicate::DepthBelowSeaIn(SEABED_SHELF_LAST_DEPTH + 1, SEABED_SLOPE_LAST_DEPTH),
            facts,
            0,
        ) {
            if depth == SEABED_SLOPE_LAST_DEPTH && dithered {
                return self.seabed.abyss_clay;
            }
            self.seabed.slope_gravel
        } else {
            // Catch-all for `d >= 25`, so the bands tile every submerged depth.
            self.seabed.abyss_clay
        }
    }

    /// Whether this column is at a ragged sea-floor band edge (`E4.7`).
    ///
    /// Keyed on `(x, z)` only, so every block of a column takes the same band
    /// and chunk order cannot change the result.
    pub(super) fn seabed_dithers(&self, facts: &ColumnFacts) -> bool {
        hash01(hash2(self.seed, self.seafloor_tag, facts.x, facts.z)) < SEABED_DITHER_SHARE
    }

    /// Whether this warm shelf column takes a hashed clay patch (`E4.7`).
    pub(super) fn shelf_clay_patch(&self, facts: &ColumnFacts) -> bool {
        hash01(hash2(self.seed, self.clay_tag, facts.x, facts.z)) < SEABED_CLAY_SHARE
    }

    /// Slot 3 material, banded by temperature (`E4.5`).
    pub(super) fn beach_layer(&self, facts: &ColumnFacts) -> Layer {
        if facts.temp < T_SNOW {
            self.cold_beach
        } else {
            self.warm_beach
        }
    }
}
