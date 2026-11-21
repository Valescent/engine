//! The tile-shaped hydrology grid: the local lattice, [`HydroTile`], and the
//! halo pinning that makes a seam bit-identical on both sides.

use super::lake::Lake;
use super::{flood, flow, invalid, lake, sea_level};
use crate::description::{HYDRO_BORDER, HYDRO_TILE, MACRO_CELL, RIVER_MIN_CELLS};
use crate::error::TerrainError;

/// Sea level, mirroring `description::SEA` (`C1.2`).
pub(crate) const SEA: i32 = 128;

/// Highest possible terrain top, mirroring `description::MAX_TERRAIN_Y` (`C1.5`).
pub(crate) const MAX_TERRAIN_Y: i32 = 352;

/// A lake is never filled above this (`D7.2`): `MAX_TERRAIN_Y - 32`.
pub(crate) const MAX_LAKE_Y: i32 = MAX_TERRAIN_Y - 32;

/// Lowest a river bed may reach (`D3.3`): `SEA - 30`, and never below `y = 1`.
///
/// `SEA - 30` is `98`, so the `y = 1` bound is implied and never the binding
/// one in v1. `flow::clamp_bed` applies both.
pub(crate) const BED_FLOOR: i32 = SEA - 30;

// `BED_FLOOR` is `98`, so the `y = 1` bound in `D3.3` never binds.
const _: () = assert!(BED_FLOOR >= 1);

/// Cells along one tile edge, excluding the halo (`HYDRO_TILE / MACRO_CELL`).
pub const TILE_CELLS: i32 = HYDRO_TILE / MACRO_CELL;

/// Halo width on each side of a tile, **in cells** (`D8.1`).
///
/// `HYDRO_BORDER` in the description is `64` **blocks**, which is
/// `64 / MACRO_CELL = 16` cells. The unit is part of the name so a future
/// reader cannot mistake the stored 160-cell grid for a 64-cell halo.
pub(crate) const HYDRO_BORDER_CELLS: i32 = HYDRO_BORDER / MACRO_CELL;

/// Compatibility alias for callers that predate the unit suffix.
///
/// Kept equal to `HYDRO_BORDER_CELLS`; new code should use the suffixed name.
pub const BORDER_CELLS: i32 = HYDRO_BORDER_CELLS;

/// Cells along one edge of the v1 whole world: `2 * WORLD_HALF_EXTENT / MACRO_CELL`.
///
/// `WORLD_HALF_EXTENT = 1024` blocks, so the world is 2048 blocks across and
/// `512` cells on a side. `WorldHydro::solve` takes an explicit `side`, so a
/// test or a larger world does not depend on this constant; the description
/// test pins it to the schema.
pub(crate) const WORLD_SIDE_CELLS: i32 = 512;

/// Cells along every axis of a solved grid, halo included: `128 + 2 * 16`.
pub(crate) const SIDE: i32 = TILE_CELLS + 2 * HYDRO_BORDER_CELLS;

/// First cell of the grid that the tile owns. The halo runs up to it.
pub(crate) const INTERIOR_START: i32 = HYDRO_BORDER_CELLS;

/// [`SIDE`] as a slice stride.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
pub(crate) const SIDE_CELLS: usize = SIDE as usize;

/// `HYDRO_BORDER_CELLS` as a slice bound.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
pub(crate) const HALO_CELLS: usize = HYDRO_BORDER_CELLS as usize;

/// Cells in one solved grid: `160 * 160`.
pub(crate) const CELL_COUNT: usize = SIDE_CELLS * SIDE_CELLS;

// `D8.1`: the tile owns 512 blocks (128 cells) and the halo is 16 cells
// (64 blocks). Both are multiples of the macro lattice, so no cell straddles a
// seam.
const _: () = assert!(HYDRO_TILE % MACRO_CELL == 0 && HYDRO_BORDER % MACRO_CELL == 0);
const _: () = assert!(HYDRO_BORDER == 64 && HYDRO_BORDER_CELLS == 16);

/// A solved hydrology tile: the six per-cell fields plus the recorded lakes.
///
/// Cell coordinates are tile-local and include the halo: `(0, 0)` is the
/// outer corner of the halo, and `INTERIOR_START` is the first cell the tile
/// owns. Row stride is `SIDE_CELLS`, and the cell index used by [`flow`] is
/// `z * SIDE_CELLS + x`.
///
/// Every field is a pure function of `(height, inflow)`, so two tiles built
/// from the same global height function agree exactly on the cells they share.
#[derive(Clone, Debug)]
pub struct HydroTile {
    height: Vec<f32>,
    filled: Vec<f32>,
    accumulation: Vec<u32>,
    receiver: Vec<Option<u32>>,
    lake_level: Vec<i32>,
    river_width: Vec<f32>,
    lakes: Vec<Lake>,
}

impl HydroTile {
    /// Runs the flood, routing, accumulation, lake and width passes.
    ///
    /// `height` and `inflow` are both [`CELL_COUNT`] cells, row-major, with the
    /// halo at the edges (cell `(0, 0)` is the halo corner), and both must
    /// describe the same global lattice: cell `(x, z)` of a tile whose interior
    /// starts at world cell `(ox, oz)` is world cell
    /// `(ox + x - HYDRO_BORDER_CELLS, oz + z - HYDRO_BORDER_CELLS)`.
    ///
    /// `inflow` holds integer upstream accumulation from the neighbouring
    /// tiles. It is zero everywhere except halo cells, where the neighbour has
    /// already counted everything that drains across the seam.
    ///
    /// # Errors
    ///
    /// Returns [`Stage::Hydrology`] with an [`ErrorKind::InvariantViolated`]
    /// when either slice has the wrong length or a height is not finite. A
    /// non-finite height is never clamped; it is rejected (`A2.3`, `S1.4`).
    pub(crate) fn solve(height: &[f32], inflow: &[u32]) -> Result<Self, TerrainError> {
        if height.len() != CELL_COUNT {
            return Err(invalid("hydro height length"));
        }
        if inflow.len() != CELL_COUNT {
            return Err(invalid("hydro inflow length"));
        }
        if !height.iter().all(|value| value.is_finite()) {
            return Err(invalid("hydro height finite"));
        }

        let filled = flood::priority_flood_from(height, SIDE_CELLS, HALO_CELLS);
        let receiver = flow::receivers(&filled, SIDE_CELLS, sea_level());
        let accumulation = flow::accumulate(&filled, &receiver, inflow, SIDE_CELLS);
        let (filled, receiver, accumulation) =
            pinned_halo(height, inflow, filled, receiver, accumulation);
        let (lake_level, lakes) = lake::find_lakes(height, &filled, SIDE_CELLS);
        let river_width: Vec<f32> = accumulation
            .iter()
            .map(|&cells| flow::river_width(cells, RIVER_MIN_CELLS, flow::W_SCALE))
            .collect();

        Ok(Self {
            height: height.to_vec(),
            filled,
            accumulation,
            receiver,
            lake_level,
            river_width,
            lakes,
        })
    }

    /// The pre-detail macro height the tile was solved from.
    #[must_use]
    pub(crate) fn height(&self, x: i32, z: i32) -> f32 {
        cell(&self.height, x, z).copied().unwrap_or(0.0)
    }

    /// The flood surface. Routing and accumulation read this, never terrain.
    #[must_use]
    pub(crate) fn filled(&self, x: i32, z: i32) -> f32 {
        cell(&self.filled, x, z).copied().unwrap_or(0.0)
    }

    /// Upstream cell count, `1` at a headwater plus `inflow`.
    #[must_use]
    pub(crate) fn accumulation(&self, x: i32, z: i32) -> u32 {
        cell(&self.accumulation, x, z).copied().unwrap_or(0)
    }

    /// D8 receiver cell index, or `None` at the sea, a halo cell, or a sink.
    #[must_use]
    pub(crate) fn receiver(&self, x: i32, z: i32) -> Option<u32> {
        cell(&self.receiver, x, z).copied().flatten()
    }

    /// Water surface of a recorded lake, or `0` when the cell is dry.
    ///
    /// A recorded level is always above [`SEA`], so `0` is unambiguous.
    #[must_use]
    pub(crate) fn lake_level(&self, x: i32, z: i32) -> i32 {
        cell(&self.lake_level, x, z).copied().unwrap_or(0)
    }

    /// Channel width in blocks at this cell, `0` below the river threshold.
    #[must_use]
    pub(crate) fn river_width(&self, x: i32, z: i32) -> f32 {
        cell(&self.river_width, x, z).copied().unwrap_or(0.0)
    }

    /// Recorded lakes, sorted by outlet cell index. Dry regions are absent.
    #[must_use]
    pub(crate) fn lakes(&self) -> &[Lake] {
        &self.lakes
    }
}

/// Returns the slice entry at a tile-local cell, or `None` off the grid.
fn cell<T>(slice: &[T], x: i32, z: i32) -> Option<&T> {
    index_of(x, z).and_then(|index| slice.get(index))
}

/// Whether a tile-local cell is in the halo rather than the interior.
fn in_halo(x: i32, z: i32) -> bool {
    x < HYDRO_BORDER_CELLS
        || z < HYDRO_BORDER_CELLS
        || x >= SIDE - HYDRO_BORDER_CELLS
        || z >= SIDE - HYDRO_BORDER_CELLS
}

/// Rewrites the halo back to boundary state after the interior passes.
///
/// The flood already pins the halo band at its input height (it is seeded
/// there), but the router and the accumulator do not know about halos: a halo
/// cell with a lower neighbour would otherwise get a receiver, and an interior
/// river draining into the halo would inflate its accumulation. Both would make
/// the published halo depend on this tile's interior, which is exactly what
/// would break seam equality (`R1.5`).
///
/// So the halo reports `height`, `1 + inflow`, and no receiver. Anything that
/// drains across the seam is the neighbour's business, resumed from its own
/// `inflow` argument (`D2.1`).
fn pinned_halo(
    height: &[f32],
    inflow: &[u32],
    mut filled: Vec<f32>,
    mut receiver: Vec<Option<u32>>,
    mut accumulation: Vec<u32>,
) -> (Vec<f32>, Vec<Option<u32>>, Vec<u32>) {
    for z in 0..SIDE {
        for x in 0..SIDE {
            if !in_halo(x, z) {
                continue;
            }
            let Some(index) = index_of(x, z) else {
                continue;
            };
            filled[index] = height[index];
            receiver[index] = None;
            accumulation[index] = inflow[index].saturating_add(1);
        }
    }
    (filled, receiver, accumulation)
}

/// Maps a tile-local cell to its slice index, rejecting coordinates that are
/// off the grid instead of wrapping or clamping them.
#[allow(clippy::cast_sign_loss)]
pub(super) fn index_of(x: i32, z: i32) -> Option<usize> {
    if x < 0 || z < 0 || x >= SIDE || z >= SIDE {
        return None;
    }
    Some(z as usize * SIDE_CELLS + x as usize)
}
