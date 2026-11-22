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
//! 1. `flood::priority_flood` raises `filled` until every cell drains:
//!    `filled[n] = max(h[n], filled[c] + EPS)`.
//! 2. `flow::receivers` runs D8 on `filled`. Flats and pits are already
//!    resolved by the flood, so every non-sea cell has a receiver and the
//!    graph is acyclic.
//! 3. `flow::accumulate` sums upstream cell counts in
//!    `(filled descending, index ascending)` order and adds integer boundary
//!    inflow, which is the only input that comes from a neighbouring tile.
//! 4. `lake::find_lakes` records a level per basin, taken from the outlet's
//!    filled height. A region that fails either gate is not recorded.
//! 5. `flow::river_width`, `flow::bed_depth` and `flow::river_influence`
//!    turn accumulation into the U profile. Applying it belongs to W4.
//!
//! # The halo is boundary state
//!
//! A solved grid is `160 x 160` cells: the `128 x 128` interior the tile owns,
//! plus `HYDRO_HYDRO_BORDER_CELLS = 16` cells (`64` blocks) of halo on every side
//! (`D8.1`). The halo is *input*, not a second interior. After
//! `HydroTile::solve` a halo cell reports its input height as its filled
//! height, `1 + inflow` as its accumulation, and `None` as its receiver. Two
//! consequences follow, and both are deliberate:
//!
//! - The values a tile publishes for the cells it shares with a neighbour are
//!   a pure function of the shared input, so two tiles agree bit-for-bit on
//!   them (`R1.5`). `flood::priority_flood_from` seeds the whole halo band
//!   at its input height, so only the interior is solved.
//! - A river that crosses a seam stops at the boundary and is resumed by the
//!   neighbour from its `inflow` argument. That is the `D2.1` contract: the
//!   caller supplies integer accumulation from the neighbour, so the result
//!   never depends on tile generation order. Tile mode is therefore
//!   order-independent by construction (`D8.2`).
//!
//! # Mode b: the whole world (`D8.2`)
//!
//! v1 uses **world mode**. `WorldHydro::solve` runs one priority-flood, one D8
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
//! `HydroTile::solve` has no world-description argument, so the sea level and
//! the terrain ceiling that the gates need are pinned here. They mirror
//! [`crate::description::SEA`] and [`crate::description::MAX_TERRAIN_Y`], and
//! `world_constants_match_the_description` fails if they ever drift apart.

use crate::error::{ErrorKind, Stage, TerrainError};

pub mod flood;
pub mod flow;
pub mod grid;
pub mod lake;
pub mod world;

#[allow(unused_imports)] // re-exported for the internal stages and unit tests
pub(crate) use flood::EPS;
#[allow(unused_imports)] // re-exported for the internal stages and unit tests
pub(crate) use flow::{D8, ORDER};
#[allow(unused_imports)] // crate-private re-exports keep the flat module paths
pub(crate) use grid::{
    BED_FLOOR, CELL_COUNT, HALO_CELLS, HYDRO_BORDER_CELLS, HydroTile, INTERIOR_START, MAX_LAKE_Y,
    MAX_TERRAIN_Y, SEA, SIDE, SIDE_CELLS, WORLD_SIDE_CELLS,
};
pub use grid::{BORDER_CELLS, TILE_CELLS};
pub use lake::Lake;
#[allow(unused_imports)] // crate-private re-exports keep the flat module paths
pub(crate) use world::OwnedWorldTile;
pub use world::{WorldHydro, WorldTileView};

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
mod tests;
