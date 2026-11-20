//! W3 - hydrology: priority-flood, D8 routing, accumulation, per-basin lakes.
//!
//! Everything here runs on the 4-block macro lattice (`B4.1`): one cell is one
//! `MACRO_CELL`-block square, so a hydrology tile is `HYDRO_TILE / MACRO_CELL`
//! cells of interior and `HYDRO_BORDER / MACRO_CELL` cells of halo on each
//! side. No step here reads a per-block field (`D2.1`); the height input is the
//! pre-detail macro height, and every output is an `f32` or an integer on that
//! same grid.
//!
//! Order of the pass (`D1.4`: before routing, single pass; `D5.1`: v1 has no
//! erosion stage to order against):
//!
//! 1. [`flood::priority_flood`] raises `filled` until every cell drains:
//!    `filled[n] = max(h[n], filled[c] + EPS)`.
//! 2. [`flow::receivers`] runs D8 on `filled`. Flats and pits are already
//!    resolved by the flood, so every non-sea cell has a receiver and the
//!    graph is acyclic.
//! 3. [`flow::accumulate`] sums upstream cell counts in
//!    `(filled descending, index ascending)` order and adds integer boundary
//!    inflow, which is the only input that comes from a neighbouring tile.
//! 4. [`lake::find_lakes`] records a level per basin, taken from the outlet's
//!    filled height. A region that fails either gate is not recorded.
//! 5. [`flow::river_width`], [`flow::bed_depth`] and [`flow::river_influence`]
//!    turn accumulation into the U profile. Applying it belongs to W4.
//!
//! # The halo is boundary state
//!
//! A solved grid is `160 x 160` cells: the `128 x 128` interior the tile owns,
//! plus `HYDRO_HYDRO_BORDER_CELLS = 16` cells (`64` blocks) of halo on every side
//! (`D8.1`). The halo is *input*, not a second interior. After
//! [`HydroTile::solve`] a halo cell reports its input height as its filled
//! height, `1 + inflow` as its accumulation, and `None` as its receiver. Two
//! consequences follow, and both are deliberate:
//!
//! - The values a tile publishes for the cells it shares with a neighbour are
//!   a pure function of the shared input, so two tiles agree bit-for-bit on
//!   them (`R1.5`). [`flood::priority_flood_from`] seeds the whole halo band
//!   at its input height, so only the interior is solved.
//! - A river that crosses a seam stops at the boundary and is resumed by the
//!   neighbour from its `inflow` argument. That is the `D2.1` contract: the
//!   caller supplies integer accumulation from the neighbour, so the result
//!   never depends on tile generation order. Tile mode is therefore
//!   order-independent by construction (`D8.2`).
//!
//! # Mode b: the whole world (`D8.2`)
//!
//! v1 uses **world mode**. [`WorldHydro::solve`] runs one priority-flood, one D8
//! pass, one accumulation and one lake pass over the entire `WORLD_HALF_EXTENT`
//! grid (512 x 512 cells for the v1 world). There is no halo and no basin-size
//! cap: a basin that crosses a 512-block tile seam is one connected region, so
//! both tiles read the exact same level. [`WorldHydro::tile_view`] slices that
//! result into the same local-cell convention [`HydroTile`] uses, without
//! copying or re-solving. Tile mode stays defined for worlds larger than one
//! whole-world pass can afford (`D8.1`).
//!
//! # World constants
//!
//! [`HydroTile::solve`] has no world-description argument, so the sea level and
//! the terrain ceiling that the gates need are pinned here. They mirror
//! [`crate::description::SEA`] and [`crate::description::MAX_TERRAIN_Y`], and
//! `world_constants_match_the_description` fails if they ever drift apart.

use crate::description::{HYDRO_BORDER, HYDRO_TILE, MACRO_CELL, RIVER_MIN_CELLS};
use crate::error::{ErrorKind, Stage, TerrainError};

pub mod flood;
pub mod flow;
pub mod lake;

pub use flood::EPS;
pub use flow::{D8, ORDER};
pub use lake::Lake;

/// Sea level, mirroring `description::SEA` (`C1.2`).
pub const SEA: i32 = 128;

/// Highest possible terrain top, mirroring `description::MAX_TERRAIN_Y` (`C1.5`).
pub const MAX_TERRAIN_Y: i32 = 352;

/// A lake is never filled above this (`D7.2`): `MAX_TERRAIN_Y - 32`.
pub const MAX_LAKE_Y: i32 = MAX_TERRAIN_Y - 32;

/// Lowest a river bed may reach (`D3.3`): `SEA - 30`, and never below `y = 1`.
///
/// `SEA - 30` is `98`, so the `y = 1` bound is implied and never the binding
/// one in v1. [`flow::clamp_bed`] applies both.
pub const BED_FLOOR: i32 = SEA - 30;

// `BED_FLOOR` is `98`, so the `y = 1` bound in `D3.3` never binds.
const _: () = assert!(BED_FLOOR >= 1);

/// Cells along one tile edge, excluding the halo (`HYDRO_TILE / MACRO_CELL`).
pub const TILE_CELLS: i32 = HYDRO_TILE / MACRO_CELL;

/// Halo width on each side of a tile, **in cells** (`D8.1`).
///
/// `HYDRO_BORDER` in the description is `64` **blocks**, which is
/// `64 / MACRO_CELL = 16` cells. The unit is part of the name so a future
/// reader cannot mistake the stored 160-cell grid for a 64-cell halo.
pub const HYDRO_BORDER_CELLS: i32 = HYDRO_BORDER / MACRO_CELL;

/// Compatibility alias for callers that predate the unit suffix.
///
/// Kept equal to [`HYDRO_BORDER_CELLS`]; new code should use the suffixed name.
pub const BORDER_CELLS: i32 = HYDRO_BORDER_CELLS;

/// Cells along one edge of the v1 whole world: `2 * WORLD_HALF_EXTENT / MACRO_CELL`.
///
/// `WORLD_HALF_EXTENT = 1024` blocks, so the world is 2048 blocks across and
/// `512` cells on a side. [`WorldHydro::solve`] takes an explicit `side`, so a
/// test or a larger world does not depend on this constant; the description
/// test pins it to the schema.
pub const WORLD_SIDE_CELLS: i32 = 512;

/// Cells along every axis of a solved grid, halo included: `128 + 2 * 16`.
pub const SIDE: i32 = TILE_CELLS + 2 * HYDRO_BORDER_CELLS;

/// First cell of the grid that the tile owns. The halo runs up to it.
pub const INTERIOR_START: i32 = HYDRO_BORDER_CELLS;

/// [`SIDE`] as a slice stride.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
pub const SIDE_CELLS: usize = SIDE as usize;

/// [`HYDRO_BORDER_CELLS`] as a slice bound.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
pub const HALO_CELLS: usize = HYDRO_BORDER_CELLS as usize;

/// Cells in one solved grid: `160 * 160`.
pub const CELL_COUNT: usize = SIDE_CELLS * SIDE_CELLS;

// `D8.1`: the tile owns 512 blocks (128 cells) and the halo is 16 cells
// (64 blocks). Both are multiples of the macro lattice, so no cell straddles a
// seam.
const _: () = assert!(HYDRO_TILE % MACRO_CELL == 0 && HYDRO_BORDER % MACRO_CELL == 0);
const _: () = assert!(HYDRO_BORDER == 64 && HYDRO_BORDER_CELLS == 16);

/// A solved hydrology tile: the six per-cell fields plus the recorded lakes.
///
/// Cell coordinates are tile-local and include the halo: `(0, 0)` is the
/// outer corner of the halo, and [`INTERIOR_START`] is the first cell the tile
/// owns. Row stride is [`SIDE_CELLS`], and the cell index used by [`flow`] is
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
    pub fn solve(height: &[f32], inflow: &[u32]) -> Result<Self, TerrainError> {
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
    pub fn height(&self, x: i32, z: i32) -> f32 {
        cell(&self.height, x, z).copied().unwrap_or(0.0)
    }

    /// The flood surface. Routing and accumulation read this, never terrain.
    #[must_use]
    pub fn filled(&self, x: i32, z: i32) -> f32 {
        cell(&self.filled, x, z).copied().unwrap_or(0.0)
    }

    /// Upstream cell count, `1` at a headwater plus `inflow`.
    #[must_use]
    pub fn accumulation(&self, x: i32, z: i32) -> u32 {
        cell(&self.accumulation, x, z).copied().unwrap_or(0)
    }

    /// D8 receiver cell index, or `None` at the sea, a halo cell, or a sink.
    #[must_use]
    pub fn receiver(&self, x: i32, z: i32) -> Option<u32> {
        cell(&self.receiver, x, z).copied().flatten()
    }

    /// Water surface of a recorded lake, or `0` when the cell is dry.
    ///
    /// A recorded level is always above [`SEA`], so `0` is unambiguous.
    #[must_use]
    pub fn lake_level(&self, x: i32, z: i32) -> i32 {
        cell(&self.lake_level, x, z).copied().unwrap_or(0)
    }

    /// Channel width in blocks at this cell, `0` below the river threshold.
    #[must_use]
    pub fn river_width(&self, x: i32, z: i32) -> f32 {
        cell(&self.river_width, x, z).copied().unwrap_or(0.0)
    }

    /// Recorded lakes, sorted by outlet cell index. Dry regions are absent.
    #[must_use]
    pub fn lakes(&self) -> &[Lake] {
        &self.lakes
    }
}

/// A solved whole-world hydrology grid (`D8.2` mode b).
///
/// One priority-flood, one D8 pass, one accumulation and one lake pass cover
/// the entire finite world, so a basin is never cut by a tile seam and there is
/// no basin-size cap. Accessors use **global cell** coordinates: cell `(x, z)`
/// covers blocks `[x * MACRO_CELL, (x + 1) * MACRO_CELL)`, the same lattice
/// [`HydroTile`] maps to.
///
/// [`WorldHydro::tile_view`] exposes the result through [`HydroTile`]'s local
/// cell convention without copying or re-solving, which is what lets a caller
/// keep one tile-shaped consumer.
#[derive(Clone, Debug)]
pub struct WorldHydro {
    side: usize,
    origin_x: i32,
    origin_z: i32,
    height: Vec<f32>,
    filled: Vec<f32>,
    accumulation: Vec<u32>,
    receiver: Vec<Option<u32>>,
    lake_level: Vec<i32>,
    river_width: Vec<f32>,
    lakes: Vec<Lake>,
}

impl WorldHydro {
    /// Solves the v1 world, centred on cell `(0, 0)`.
    ///
    /// The grid spans `side` cells on each axis and starts at cell
    /// `-(side / 2)`, so world cell `0` is the middle of the array. Use
    /// [`WorldHydro::solve_at`] for an asymmetric origin.
    ///
    /// # Errors
    ///
    /// See [`WorldHydro::solve_at`].
    pub fn solve(height: &[f32], side: usize) -> Result<Self, TerrainError> {
        let half = i32::try_from(side / 2).unwrap_or(i32::MAX);
        Self::solve_at(-half, -half, height, side)
    }

    /// Solves a whole world whose grid starts at global cell `(origin_x, origin_z)`.
    ///
    /// # Errors
    ///
    /// Returns [`Stage::Hydrology`] with an [`ErrorKind::InvariantViolated`]
    /// when `side` is zero or overflows, when `height` is not exactly
    /// `side * side` cells, or when a height is not finite. Non-finite input is
    /// rejected, never clamped (`A2.3`, `S1.4`).
    pub fn solve_at(
        origin_x: i32,
        origin_z: i32,
        height: &[f32],
        side: usize,
    ) -> Result<Self, TerrainError> {
        let Some(cells) = side.checked_mul(side) else {
            return Err(invalid("world hydro side"));
        };
        if side == 0 || height.len() != cells {
            return Err(invalid("world hydro height length"));
        }
        if !height.iter().all(|value| value.is_finite()) {
            return Err(invalid("world hydro height finite"));
        }

        // The outer ring is the open boundary of the finite world. The edge
        // ocean mask keeps it at or below sea in v1, so it drains there.
        let filled = flood::priority_flood(height, side);
        let receiver = flow::receivers(&filled, side, sea_level());
        let inflow = vec![0_u32; cells];
        let accumulation = flow::accumulate(&filled, &receiver, &inflow, side);
        let (lake_level, lakes) = lake::find_lakes(height, &filled, side);
        let river_width: Vec<f32> = accumulation
            .iter()
            .map(|&count| flow::river_width(count, RIVER_MIN_CELLS, flow::W_SCALE))
            .collect();

        Ok(Self {
            side,
            origin_x,
            origin_z,
            height: height.to_vec(),
            filled,
            accumulation,
            receiver,
            lake_level,
            river_width,
            lakes,
        })
    }

    /// Pre-detail macro height at a global cell.
    #[must_use]
    pub fn height(&self, x: i32, z: i32) -> f32 {
        self.get(&self.height, x, z).copied().unwrap_or(0.0)
    }

    /// Flood surface at a global cell.
    #[must_use]
    pub fn filled(&self, x: i32, z: i32) -> f32 {
        self.get(&self.filled, x, z).copied().unwrap_or(0.0)
    }

    /// Upstream cell count at a global cell.
    #[must_use]
    pub fn accumulation(&self, x: i32, z: i32) -> u32 {
        self.get(&self.accumulation, x, z).copied().unwrap_or(0)
    }

    /// D8 receiver at a global cell, as a global grid index.
    #[must_use]
    pub fn receiver(&self, x: i32, z: i32) -> Option<u32> {
        self.get(&self.receiver, x, z).copied().flatten()
    }

    /// Lake surface at a global cell, or `0` when dry.
    #[must_use]
    pub fn lake_level(&self, x: i32, z: i32) -> i32 {
        self.get(&self.lake_level, x, z).copied().unwrap_or(0)
    }

    /// Channel width in blocks at a global cell.
    #[must_use]
    pub fn river_width(&self, x: i32, z: i32) -> f32 {
        self.get(&self.river_width, x, z).copied().unwrap_or(0.0)
    }

    /// Every recorded lake, sorted by outlet index. Cells and outlets are
    /// indices into this world grid, never tile-local.
    #[must_use]
    pub fn lakes(&self) -> &[Lake] {
        &self.lakes
    }

    /// Cells along one edge of the solved grid.
    #[must_use]
    pub fn side(&self) -> usize {
        self.side
    }

    /// The global cell at grid index `index`, or `None` off the grid.
    #[must_use]
    pub fn cell_of(&self, index: u32) -> Option<(i32, i32)> {
        let index = usize::try_from(index).ok()?;
        if index >= self.height.len() {
            return None;
        }
        let x = i32::try_from(index % self.side).ok()?;
        let z = i32::try_from(index / self.side).ok()?;
        Some((x.checked_add(self.origin_x)?, z.checked_add(self.origin_z)?))
    }

    /// A tile-shaped view of this world result, or `None` when the tile is not
    /// wholly inside the solved world.
    ///
    /// The view uses [`HydroTile`]'s convention: local `x`/`z` in
    /// `0..SIDE`, the owned square starting at [`INTERIOR_START`], and the
    /// cells just outside it reading the neighbouring world cells. Because the
    /// world pass already solved every cell exactly, the view never re-solves
    /// and has no halo pinning.
    #[must_use]
    pub fn tile_view(&self, tile_x: i32, tile_z: i32) -> Option<WorldTileView<'_>> {
        let view = WorldTileView {
            world: self,
            origin_x: tile_x.checked_mul(TILE_CELLS)?,
            origin_z: tile_z.checked_mul(TILE_CELLS)?,
        };
        // Require the whole owned square, not just its first cell.
        let end_x = view.origin_x.checked_add(TILE_CELLS)?.checked_sub(1)?;
        let end_z = view.origin_z.checked_add(TILE_CELLS)?.checked_sub(1)?;
        if self.index_of(view.origin_x, view.origin_z).is_some()
            && self.index_of(end_x, end_z).is_some()
        {
            Some(view)
        } else {
            None
        }
    }

    /// Maps a global cell to its grid index, rejecting off-grid coordinates.
    fn index_of(&self, x: i32, z: i32) -> Option<usize> {
        let lx = i64::from(x) - i64::from(self.origin_x);
        let lz = i64::from(z) - i64::from(self.origin_z);
        let side = i64::try_from(self.side).ok()?;
        if lx < 0 || lz < 0 || lx >= side || lz >= side {
            return None;
        }
        let x = usize::try_from(lx).ok()?;
        let z = usize::try_from(lz).ok()?;
        Some(z * self.side + x)
    }

    /// Slice entry at a global cell.
    fn get<'a, T>(&'a self, slice: &'a [T], x: i32, z: i32) -> Option<&'a T> {
        self.index_of(x, z).and_then(|index| slice.get(index))
    }
}

/// One tile of a [`WorldHydro`] result, addressed in [`HydroTile`] coordinates.
///
/// Owned cells are local `HYDRO_BORDER_CELLS .. SIDE - HYDRO_BORDER_CELLS`.
/// Unlike [`HydroTile`], the halo cells around them are the world's own solved
/// cells, so a view and the world agree on every field, not just on the owned
/// square.
#[derive(Clone, Copy, Debug)]
pub struct WorldTileView<'a> {
    world: &'a WorldHydro,
    /// Global cell of the first owned cell, not of the local origin.
    origin_x: i32,
    origin_z: i32,
}

impl WorldTileView<'_> {
    /// The global cell a local view cell refers to.
    fn global(&self, x: i32, z: i32) -> Option<(i32, i32)> {
        Some((
            self.origin_x
                .checked_add(x)?
                .checked_sub(HYDRO_BORDER_CELLS)?,
            self.origin_z
                .checked_add(z)?
                .checked_sub(HYDRO_BORDER_CELLS)?,
        ))
    }

    /// Grid index of a local view cell, or `None` off the view or the world.
    fn index_of(&self, x: i32, z: i32) -> Option<usize> {
        if x < 0 || z < 0 || x >= SIDE || z >= SIDE {
            return None;
        }
        let (global_x, global_z) = self.global(x, z)?;
        self.world.index_of(global_x, global_z)
    }

    /// Slice entry at one local view cell.
    fn get<'a, T>(&'a self, slice: &'a [T], x: i32, z: i32) -> Option<&'a T> {
        self.index_of(x, z).and_then(|index| slice.get(index))
    }

    /// Pre-detail macro height at a local cell.
    #[must_use]
    pub fn height(&self, x: i32, z: i32) -> f32 {
        self.get(&self.world.height, x, z).copied().unwrap_or(0.0)
    }

    /// Flood surface at a local cell.
    #[must_use]
    pub fn filled(&self, x: i32, z: i32) -> f32 {
        self.get(&self.world.filled, x, z).copied().unwrap_or(0.0)
    }

    /// Upstream cell count at a local cell.
    #[must_use]
    pub fn accumulation(&self, x: i32, z: i32) -> u32 {
        self.get(&self.world.accumulation, x, z)
            .copied()
            .unwrap_or(0)
    }

    /// D8 receiver at a local cell, as a global grid index.
    #[must_use]
    pub fn receiver(&self, x: i32, z: i32) -> Option<u32> {
        self.get(&self.world.receiver, x, z).copied().flatten()
    }

    /// Lake surface at a local cell, or `0` when dry.
    #[must_use]
    pub fn lake_level(&self, x: i32, z: i32) -> i32 {
        self.get(&self.world.lake_level, x, z).copied().unwrap_or(0)
    }

    /// Channel width in blocks at a local cell.
    #[must_use]
    pub fn river_width(&self, x: i32, z: i32) -> f32 {
        self.get(&self.world.river_width, x, z)
            .copied()
            .unwrap_or(0.0)
    }

    /// World lakes that touch this tile, with **global** cell indices.
    pub fn lakes(&self) -> impl Iterator<Item = &Lake> {
        let start_x = self.origin_x;
        let start_z = self.origin_z;
        let end_x = start_x.saturating_add(TILE_CELLS);
        let end_z = start_z.saturating_add(TILE_CELLS);
        self.world.lakes.iter().filter(move |lake| {
            lake.cells.iter().any(|&index| {
                self.world.cell_of(index).is_some_and(|(x, z)| {
                    (start_x..end_x).contains(&x) && (start_z..end_z).contains(&z)
                })
            })
        })
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
fn index_of(x: i32, z: i32) -> Option<usize> {
    if x < 0 || z < 0 || x >= SIDE || z >= SIDE {
        return None;
    }
    Some(z as usize * SIDE_CELLS + x as usize)
}

/// Sea level as a routing threshold. `128` is exact in `f32`.
#[allow(clippy::cast_precision_loss)]
fn sea_level() -> f32 {
    SEA as f32
}

/// `floor` to `i32`: the single quantiser for hydrology levels (`C3.1`).
///
/// `f32 as i32` saturates in Rust, so a wild height clamps instead of wrapping.
#[allow(clippy::cast_possible_truncation)]
fn floor_i32(value: f32) -> i32 {
    value.floor() as i32
}

/// Builds a hydrology-stage invariant error.
fn invalid(check: &'static str) -> TerrainError {
    TerrainError::new(
        Stage::Hydrology,
        ErrorKind::InvariantViolated { check, coord: None },
    )
}

#[cfg(test)]
pub(crate) mod fixture {
    //! Deterministic helpers shared by the hydrology tests.
    //!
    //! These never touch world output: they are a test-only shuffle and a
    //! chaotic height function, both integer-only so a failure reproduces.

    /// `SplitMix64`, used only to shuffle inputs and build test grids.
    pub(crate) struct Rng(u64);

    impl Rng {
        pub(crate) const fn new(seed: u64) -> Self {
            Self(seed)
        }

        pub(crate) fn next_u32(&mut self) -> u32 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut value = self.0;
            value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            value ^= value >> 31;
            u32::try_from(value >> 32).unwrap_or(0)
        }

        pub(crate) fn below(&mut self, bound: u32) -> u32 {
            if bound == 0 {
                0
            } else {
                self.next_u32() % bound
            }
        }

        pub(crate) fn index_below(&mut self, bound: usize) -> usize {
            usize::try_from(self.below(u32::try_from(bound).unwrap_or(u32::MAX))).unwrap_or(0)
        }

        pub(crate) fn shuffle<T>(&mut self, items: &mut [T]) {
            for index in (1..items.len()).rev() {
                let other = self.index_below(index + 1);
                items.swap(index, other);
            }
        }
    }

    /// A chaotic but deterministic height in `[40, 200)` blocks.
    ///
    /// Smooth functions would hide an off-by-one in the halo, so the test tile
    /// is built from a hash.
    pub(crate) fn terrain(x: i32, z: i32) -> f32 {
        let mut value = (u64::from(x.cast_unsigned()) << 32) ^ u64::from(z.cast_unsigned());
        value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        value ^= value >> 31;
        f32::from(u16::try_from(value % 160).unwrap_or(0)) + 40.0
    }

    /// A halo inflow in `0..20`, keyed on the world cell so two tiles agree.
    pub(crate) fn inflow(x: i32, z: i32) -> u32 {
        let mut value = u64::from(x.cast_unsigned()).wrapping_mul(0xD6E8_FEB8_6659_FD93);
        value ^= u64::from(z.cast_unsigned()).wrapping_mul(0xA24B_AED4_963E_E407);
        value ^= value >> 29;
        u32::try_from(value % 20).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{inflow, terrain};
    use super::*;

    /// Fills a grid for a tile whose interior starts at world cell `(ox, oz)`.
    fn grid(ox: i32, oz: i32) -> Vec<f32> {
        let mut height = vec![0.0_f32; CELL_COUNT];
        for z in 0..SIDE {
            for x in 0..SIDE {
                let index = index_of(x, z).unwrap();
                height[index] = terrain(ox + x - HYDRO_BORDER_CELLS, oz + z - HYDRO_BORDER_CELLS);
            }
        }
        height
    }

    /// Halo-only inflow for the same tile.
    fn inflow_grid(ox: i32, oz: i32) -> Vec<u32> {
        let mut result = vec![0_u32; CELL_COUNT];
        for z in 0..SIDE {
            for x in 0..SIDE {
                let halo = x < HYDRO_BORDER_CELLS
                    || z < HYDRO_BORDER_CELLS
                    || x >= SIDE - HYDRO_BORDER_CELLS
                    || z >= SIDE - HYDRO_BORDER_CELLS;
                if halo {
                    let index = index_of(x, z).unwrap();
                    result[index] =
                        inflow(ox + x - HYDRO_BORDER_CELLS, oz + z - HYDRO_BORDER_CELLS);
                }
            }
        }
        result
    }

    #[test]
    fn grid_shape_matches_the_pinned_tile() {
        assert_eq!(TILE_CELLS, 128);
        assert_eq!(HYDRO_BORDER_CELLS, 16, "the halo is 16 cells = 64 blocks");
        assert_eq!(BORDER_CELLS, HYDRO_BORDER_CELLS, "alias stays in sync");
        assert_eq!(SIDE, 160);
        assert_eq!(CELL_COUNT, 25_600);
        assert_eq!(INTERIOR_START, 16);
    }

    #[test]
    fn the_world_grid_covers_the_description() {
        let half_extent_cells = crate::description::default_v1().world_half_extent / MACRO_CELL;
        assert_eq!(WORLD_SIDE_CELLS, 2 * half_extent_cells);
        assert_eq!(WORLD_SIDE_CELLS as usize % TILE_CELLS as usize, 0);
        assert_eq!(WORLD_SIDE_CELLS / TILE_CELLS, 4, "four tiles per axis");
    }

    #[test]
    fn world_constants_match_the_description() {
        assert_eq!(SEA, crate::description::SEA);
        assert_eq!(MAX_TERRAIN_Y, crate::description::MAX_TERRAIN_Y);
        assert_eq!(MAX_LAKE_Y, crate::description::MAX_TERRAIN_Y - 32);
        assert_eq!(BED_FLOOR, crate::description::SEA - 30);
    }

    #[test]
    fn solve_rejects_malformed_input() {
        let good = vec![50.0_f32; CELL_COUNT];
        let inflow = vec![0_u32; CELL_COUNT];

        let error = HydroTile::solve(&good[..CELL_COUNT - 1], &inflow).unwrap_err();
        assert_eq!(error.stage, Stage::Hydrology);
        assert!(matches!(error.kind, ErrorKind::InvariantViolated { .. }));

        let error = HydroTile::solve(&good, &inflow[..10]).unwrap_err();
        assert_eq!(error.stage, Stage::Hydrology);

        let mut bad = good;
        bad[123] = f32::NAN;
        let error = HydroTile::solve(&bad, &inflow).unwrap_err();
        assert!(matches!(error.kind, ErrorKind::InvariantViolated { .. }));
    }

    #[test]
    fn a_flat_sea_reports_no_river_and_no_lake() {
        let height = vec![50.0_f32; CELL_COUNT];
        let tile = HydroTile::solve(&height, &vec![0_u32; CELL_COUNT]).unwrap();

        for (x, z) in [(0, 0), (80, 80), (SIDE - 1, SIDE - 1)] {
            assert_eq!(tile.height(x, z), 50.0);
            assert_eq!(tile.filled(x, z), 50.0);
            assert_eq!(tile.accumulation(x, z), 1);
            assert_eq!(tile.receiver(x, z), None);
            assert_eq!(tile.lake_level(x, z), 0);
            assert_eq!(tile.river_width(x, z), 0.0);
        }
        assert!(tile.lakes().is_empty(), "no lake in a flat sea");
    }

    #[test]
    fn off_grid_accessors_return_defaults() {
        let height = vec![50.0_f32; CELL_COUNT];
        let tile = HydroTile::solve(&height, &vec![0_u32; CELL_COUNT]).unwrap();

        for (x, z) in [(-1, 0), (0, -1), (SIDE, 0), (0, SIDE), (i32::MIN, i32::MAX)] {
            assert_eq!(tile.height(x, z), 0.0);
            assert_eq!(tile.filled(x, z), 0.0);
            assert_eq!(tile.accumulation(x, z), 0);
            assert_eq!(tile.receiver(x, z), None);
            assert_eq!(tile.lake_level(x, z), 0);
            assert_eq!(tile.river_width(x, z), 0.0);
        }
    }

    /// `R1.5`, `D1.3`: the cells two tiles share must come out bit-identical.
    ///
    /// A tile shifted by `TILE_CELLS - HYDRO_BORDER_CELLS` cells puts its left halo
    /// exactly on this tile's right halo, so every published field must agree
    /// there, on both axes. A second comparison against a neighbour one tile
    /// away checks that the coordinate convention itself is right: heights are
    /// the same global function in both grids.
    #[test]
    fn hydro_tile_is_halo_consistent() {
        let origin = HydroTile::solve(&grid(0, 0), &inflow_grid(0, 0)).unwrap();
        // A tile whose interior starts `one tile plus one halo` further along
        // puts its left halo exactly on this tile's right halo.
        let shift = TILE_CELLS + HYDRO_BORDER_CELLS;

        // x axis: the right halo of `origin` is the left halo of this tile.
        let east = HydroTile::solve(&grid(shift, 0), &inflow_grid(shift, 0)).unwrap();
        for z in 0..SIDE {
            for x in SIDE - HYDRO_BORDER_CELLS..SIDE {
                assert_same_cell(&origin, x, z, &east, x - shift, z, "x halo");
            }
        }

        // z axis, same construction.
        let south = HydroTile::solve(&grid(0, shift), &inflow_grid(0, shift)).unwrap();
        for z in SIDE - HYDRO_BORDER_CELLS..SIDE {
            for x in 0..SIDE {
                assert_same_cell(&origin, x, z, &south, x, z - shift, "z halo");
            }
        }

        // One tile east: the overlap is this tile's interior against the
        // neighbour's halo. Heights must be the same global samples, which
        // pins the halo placement to the cell (no half-cell or 16-cell slip).
        let neighbour =
            HydroTile::solve(&grid(TILE_CELLS, 0), &inflow_grid(TILE_CELLS, 0)).unwrap();
        for z in 0..SIDE {
            for x in TILE_CELLS..SIDE {
                assert_eq!(
                    origin.height(x, z).to_bits(),
                    neighbour.height(x - TILE_CELLS, z).to_bits(),
                    "height at {x},{z} across the tile seam"
                );
            }
        }
    }

    /// Fills a whole-world grid for `side` cells starting at global cell
    /// `(origin_x, origin_z)`.
    fn world_grid(origin_x: i32, origin_z: i32, side: usize) -> Vec<f32> {
        let mut height = vec![0.0_f32; side * side];
        for z in 0..side {
            for x in 0..side {
                let global_x = origin_x + i32::try_from(x).expect("the fixture fits i32");
                let global_z = origin_z + i32::try_from(z).expect("the fixture fits i32");
                height[z * side + x] = terrain(global_x, global_z);
            }
        }
        height
    }

    /// `D8.2` mode b: a tile view is the world result, cell for cell.
    #[test]
    fn world_view_equals_the_world_on_owned_cells() {
        let side = usize::try_from(TILE_CELLS * 2).expect("positive");
        let origin = -TILE_CELLS;
        let world =
            WorldHydro::solve(&world_grid(origin, origin, side), side).expect("valid world");

        for tile_z in [-1, 0] {
            for tile_x in [-1, 0] {
                let view = world
                    .tile_view(tile_x, tile_z)
                    .expect("a tile wholly inside the world has a view");
                for z in HYDRO_BORDER_CELLS..SIDE - HYDRO_BORDER_CELLS {
                    for x in HYDRO_BORDER_CELLS..SIDE - HYDRO_BORDER_CELLS {
                        let global_x = tile_x * TILE_CELLS + x - HYDRO_BORDER_CELLS;
                        let global_z = tile_z * TILE_CELLS + z - HYDRO_BORDER_CELLS;
                        assert_eq!(
                            view.height(x, z).to_bits(),
                            world.height(global_x, global_z).to_bits(),
                            "height at {global_x},{global_z}"
                        );
                        assert_eq!(
                            view.filled(x, z).to_bits(),
                            world.filled(global_x, global_z).to_bits(),
                            "filled at {global_x},{global_z}"
                        );
                        assert_eq!(
                            view.accumulation(x, z),
                            world.accumulation(global_x, global_z),
                            "accumulation at {global_x},{global_z}"
                        );
                        assert_eq!(
                            view.receiver(x, z),
                            world.receiver(global_x, global_z),
                            "receiver at {global_x},{global_z}"
                        );
                        assert_eq!(
                            view.lake_level(x, z),
                            world.lake_level(global_x, global_z),
                            "lake level at {global_x},{global_z}"
                        );
                        assert_eq!(
                            view.river_width(x, z).to_bits(),
                            world.river_width(global_x, global_z).to_bits(),
                            "river width at {global_x},{global_z}"
                        );
                    }
                }
            }
        }

        // A tile that is not wholly inside the world has no view.
        assert!(world.tile_view(-2, 0).is_none());
        assert!(world.tile_view(1, 0).is_none());
        assert!(world.tile_view(0, 1).is_none());
    }

    /// `D8.2` mode b: no basin cap, so a basin across a seam has one level.
    #[test]
    fn a_basin_straddling_a_tile_edge_gets_the_same_level_from_both_tiles() {
        let side = usize::try_from(TILE_CELLS * 2).expect("positive");
        let half = TILE_CELLS;
        // A closed bowl on a steep cone. The rim is at Chebyshev radius 3..4
        // around the bowl centre; everything outside descends monotonically to
        // sea, so the only depression is the bowl. The bowl spans x = -2..=2,
        // which straddles the x = 0 tile seam.
        let mut height = vec![60.0_f32; side * side];
        for z in 0..side {
            for x in 0..side {
                let global_x = -half + i32::try_from(x).expect("fits");
                let global_z = -half + i32::try_from(z).expect("fits");
                let radius = global_x.abs().max((global_z - 12).abs());
                let value = if radius <= 2 {
                    190.0
                } else if radius <= 4 {
                    200.0
                } else {
                    #[allow(clippy::cast_precision_loss)]
                    let distance = (radius - 4) as f32;
                    (200.0 - 8.0 * distance).max(60.0)
                };
                height[z * side + x] = value;
            }
        }

        let world = WorldHydro::solve(&height, side).expect("valid world");
        let west = world.tile_view(-1, 0).expect("west tile");
        let east = world.tile_view(0, 0).expect("east tile");

        // The seam itself: global cells -1 and 0 are the last owned cell of
        // the west tile and the first of the east tile.
        let west_seam = world.lake_level(-1, 12);
        let east_seam = world.lake_level(0, 12);
        assert_eq!(west_seam, 200, "the bowl spills over its 200 rim");
        assert_eq!(west_seam, east_seam, "the seam must not split the basin");

        // Every bowl cell reports the same level from whichever tile owns it.
        for global_z in 10..=14 {
            for global_x in -2..=2 {
                let (view, local_x) = if global_x < 0 {
                    (&west, global_x + TILE_CELLS + HYDRO_BORDER_CELLS)
                } else {
                    (&east, global_x + HYDRO_BORDER_CELLS)
                };
                let local_z = global_z + HYDRO_BORDER_CELLS;
                assert_eq!(
                    world.lake_level(global_x, global_z),
                    200,
                    "world level at {global_x},{global_z}"
                );
                assert_eq!(
                    view.lake_level(local_x, local_z),
                    world.lake_level(global_x, global_z),
                    "view level at {global_x},{global_z}"
                );
            }
        }
    }

    #[test]
    fn world_solve_rejects_malformed_input() {
        assert!(WorldHydro::solve(&[], 0).is_err(), "zero side");
        assert!(WorldHydro::solve(&[1.0; 3], 2).is_err(), "wrong length");

        let mut bad = vec![50.0_f32; 16];
        bad[5] = f32::INFINITY;
        let error = WorldHydro::solve(&bad, 4).expect_err("non-finite is rejected");
        assert_eq!(error.stage, Stage::Hydrology);
        assert!(matches!(error.kind, ErrorKind::InvariantViolated { .. }));
    }

    fn assert_same_cell(
        left: &HydroTile,
        lx: i32,
        lz: i32,
        right: &HydroTile,
        rx: i32,
        rz: i32,
        axis: &str,
    ) {
        assert_eq!(
            left.height(lx, lz).to_bits(),
            right.height(rx, rz).to_bits(),
            "height {axis} at {lx},{lz}"
        );
        assert_eq!(
            left.filled(lx, lz).to_bits(),
            right.filled(rx, rz).to_bits(),
            "filled {axis} at {lx},{lz}"
        );
        assert_eq!(
            left.accumulation(lx, lz),
            right.accumulation(rx, rz),
            "accumulation {axis} at {lx},{lz}"
        );
        assert_eq!(
            left.receiver(lx, lz),
            right.receiver(rx, rz),
            "receiver {axis} at {lx},{lz}"
        );
        assert_eq!(
            left.lake_level(lx, lz),
            right.lake_level(rx, rz),
            "lake level {axis} at {lx},{lz}"
        );
        assert_eq!(
            left.river_width(lx, lz).to_bits(),
            right.river_width(rx, rz).to_bits(),
            "river width {axis} at {lx},{lz}"
        );
    }
}
