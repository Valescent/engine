//! Whole-world hydrology (`D8.2` mode b): [`WorldHydro`] and its tile views.

use std::sync::Arc;

use super::grid::{HYDRO_BORDER_CELLS, SIDE, TILE_CELLS};
use super::lake::Lake;
use super::{flood, flow, invalid, lake, sea_level};
use crate::description::RIVER_MIN_CELLS;
use crate::error::TerrainError;

/// A solved whole-world hydrology grid (`D8.2` mode b).
///
/// One priority-flood, one D8 pass, one accumulation and one lake pass cover
/// the entire finite world, so a basin is never cut by a tile seam and there is
/// no basin-size cap. Accessors use **global cell** coordinates: cell `(x, z)`
/// covers blocks `[x * MACRO_CELL, (x + 1) * MACRO_CELL)`, the same lattice
/// [`HydroTile`](super::HydroTile) maps to.
///
/// [`WorldHydro::tile_view`] exposes the result through [`HydroTile`](super::HydroTile)'s local
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
    /// The view uses [`HydroTile`](super::HydroTile)'s convention: local `x`/`z` in
    /// `0..SIDE`, the owned square starting at [`INTERIOR_START`](super::grid::INTERIOR_START), and the
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

/// One tile of a [`WorldHydro`] result, addressed in [`HydroTile`](super::HydroTile) coordinates.
///
/// Owned cells are local `HYDRO_BORDER_CELLS .. SIDE - HYDRO_BORDER_CELLS`.
/// Unlike [`HydroTile`](super::HydroTile), the halo cells around them are the world's own solved
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

/// An owned tile-shaped view of a solved [`WorldHydro`].
///
/// [`WorldTileView`] borrows the solved grid; a cache needs to keep the grid
/// alive without a lifetime parameter, so this holds the same view over an
/// [`Arc`]. Local cell coordinates are identical to [`HydroTile`](super::HydroTile)'s and to
/// [`WorldTileView`]'s.
#[derive(Clone, Debug)]
pub struct OwnedWorldTile {
    world: Arc<WorldHydro>,
    /// Global cell of the first owned cell, not of the local origin.
    origin_x: i32,
    origin_z: i32,
}

impl OwnedWorldTile {
    /// Wraps `tile_x`/`tile_z`, or returns `None` when the tile is not wholly
    /// inside the solved world.
    #[must_use]
    pub fn new(world: Arc<WorldHydro>, tile_x: i32, tile_z: i32) -> Option<Self> {
        world.tile_view(tile_x, tile_z)?;
        Some(Self {
            origin_x: tile_x.checked_mul(TILE_CELLS)?,
            origin_z: tile_z.checked_mul(TILE_CELLS)?,
            world,
        })
    }

    fn view(&self) -> WorldTileView<'_> {
        WorldTileView {
            world: &self.world,
            origin_x: self.origin_x,
            origin_z: self.origin_z,
        }
    }

    /// Pre-detail macro height at a local cell.
    #[must_use]
    pub fn height(&self, x: i32, z: i32) -> f32 {
        self.view().height(x, z)
    }

    /// Flood surface at a local cell.
    #[must_use]
    pub fn filled(&self, x: i32, z: i32) -> f32 {
        self.view().filled(x, z)
    }

    /// Lake surface at a local cell, or `0` when dry.
    #[must_use]
    pub fn lake_level(&self, x: i32, z: i32) -> i32 {
        self.view().lake_level(x, z)
    }

    /// Channel width in blocks at a local cell.
    #[must_use]
    pub fn river_width(&self, x: i32, z: i32) -> f32 {
        self.view().river_width(x, z)
    }
}
