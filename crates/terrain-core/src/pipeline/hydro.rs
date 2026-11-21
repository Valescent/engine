//! The world hydrology handle for the column pipeline.
//!
//! [`HydroTileRef`] is the per-tile view the column path reads. World mode
//! solves the finite world once; tiles outside that grid fall back to a
//! standalone solve. The split is organisational only.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use crate::description::MACRO_CELL;
use crate::error::{Stage, TerrainError};
use crate::hydro::{self, HydroTile};

use super::terrain::{Terrain, micros};

/// The hydrology result for one tile (`D8.2`).
///
/// World mode is the v1 path: the tile is a view of one solve over
/// [`hydro::WORLD_SIDE_CELLS`], so a basin that crosses a tile seam has one
/// level. Tile mode remains for tiles outside that grid.
#[derive(Clone, Debug)]
pub enum HydroTileRef {
    /// A tile solved on its own, with zero boundary inflow.
    Tile(Rc<HydroTile>),
    /// A tile-shaped view of the whole-world solve.
    World(hydro::OwnedWorldTile),
}

impl HydroTileRef {
    /// Pre-detail macro height at a local cell.
    #[must_use]
    pub fn height(&self, x: i32, z: i32) -> f32 {
        match self {
            Self::Tile(tile) => tile.height(x, z),
            Self::World(tile) => tile.height(x, z),
        }
    }

    /// Lake surface at a local cell, or `0` when dry.
    #[must_use]
    pub fn lake_level(&self, x: i32, z: i32) -> i32 {
        match self {
            Self::Tile(tile) => tile.lake_level(x, z),
            Self::World(tile) => tile.lake_level(x, z),
        }
    }

    /// Channel width in blocks at a local cell.
    #[must_use]
    pub fn river_width(&self, x: i32, z: i32) -> f32 {
        match self {
            Self::Tile(tile) => tile.river_width(x, z),
            Self::World(tile) => tile.river_width(x, z),
        }
    }
}

impl Terrain {
    /// Hydro tile covering a 512-block square.
    ///
    /// v1 uses world mode (`D8.2`): the first call solves the whole finite
    /// world once and every tile reads a view of that result, so a basin is
    /// never cut at a tile seam. A tile outside the solved world falls back to a
    /// standalone solve with zero boundary inflow.
    ///
    /// # Errors
    ///
    /// Propagates `WorldHydro::solve` and `HydroTile::solve` failures.
    pub fn hydro_tile(&self, hydro_x: i32, hydro_z: i32) -> Result<HydroTileRef, TerrainError> {
        if let Some(cached) = self.hydro.borrow().get(&(hydro_x, hydro_z)) {
            return Ok(cached.clone());
        }
        let hydro_started = Instant::now();
        let world = self.world_hydro()?;
        let tile =
            if let Some(view) = hydro::OwnedWorldTile::new(Arc::clone(&world), hydro_x, hydro_z) {
                HydroTileRef::World(view)
            } else {
                let side = hydro::SIDE;
                let heights = self.tile_heights(hydro_x, hydro_z);
                let inflow = vec![0_u32; (side * side) as usize];
                HydroTileRef::Tile(Rc::new(HydroTile::solve(&heights, &inflow)?))
            };
        // Diagnostics only, taken after the solve returned.
        self.counters
            .record_latency(Stage::Hydrology, micros(hydro_started));
        self.hydro
            .borrow_mut()
            .insert((hydro_x, hydro_z), tile.clone());
        Ok(tile)
    }

    /// Installs one solved world, so every worker shares a single solve.
    ///
    /// The column memo is cleared because water depths and lake levels change
    /// with the hydrology result.
    #[must_use]
    pub fn with_world_hydro(mut self, world: Arc<hydro::WorldHydro>) -> Self {
        *self.world_hydro.get_mut() = Some(world);
        self.column_cache.get_mut().entries.clear();
        self.column_cache.get_mut().order.clear();
        self
    }

    /// Solves the whole finite world at [`hydro::WORLD_SIDE_CELLS`].
    ///
    /// # Errors
    ///
    /// Propagates [`hydro::WorldHydro::solve`].
    pub fn solve_world_hydro(&self) -> Result<Arc<hydro::WorldHydro>, TerrainError> {
        let side = hydro::WORLD_SIDE_CELLS;
        let cells = usize::try_from(side * side).expect("the world grid fits usize");
        let half = side / 2;
        let mut heights = vec![0.0_f32; cells];
        for z in 0..side {
            for x in 0..side {
                let cell_x = x - half;
                let cell_z = z - half;
                let block_x = f32::from(i16::try_from(cell_x * MACRO_CELL + 2).unwrap_or(0));
                let block_z = f32::from(i16::try_from(cell_z * MACRO_CELL + 2).unwrap_or(0));
                heights[(x + side * z) as usize] = self.macro_height(block_x, block_z);
            }
        }
        Ok(Arc::new(hydro::WorldHydro::solve(
            &heights,
            usize::try_from(side).expect("the world grid side fits usize"),
        )?))
    }

    /// The cached whole-world result, solved on first use.
    fn world_hydro(&self) -> Result<Arc<hydro::WorldHydro>, TerrainError> {
        if let Some(world) = self.world_hydro.borrow().as_ref() {
            return Ok(Arc::clone(world));
        }
        let world = self.solve_world_hydro()?;
        *self.world_hydro.borrow_mut() = Some(Arc::clone(&world));
        Ok(world)
    }

    /// Heights for one standalone tile, halo included.
    fn tile_heights(&self, hydro_x: i32, hydro_z: i32) -> Vec<f32> {
        let side = hydro::SIDE;
        let mut heights = vec![0.0_f32; (side * side) as usize];
        for z in 0..side {
            for x in 0..side {
                let cell_x = hydro_x * hydro::TILE_CELLS + x - hydro::BORDER_CELLS;
                let cell_z = hydro_z * hydro::TILE_CELLS + z - hydro::BORDER_CELLS;
                let block_x = f32::from(i16::try_from(cell_x * MACRO_CELL + 2).unwrap_or(0));
                let block_z = f32::from(i16::try_from(cell_z * MACRO_CELL + 2).unwrap_or(0));
                heights[(x + side * z) as usize] = self.macro_height(block_x, block_z);
            }
        }
        heights
    }
}

pub(super) fn river_level(hydro: &HydroTileRef, hydro_x: i32, hydro_z: i32) -> i32 {
    let width = hydro.river_width(hydro_x, hydro_z);
    if width <= 0.5 {
        return i32::MIN;
    }
    crate::hydro::flow::river_level(hydro.height(hydro_x, hydro_z))
}

/// Hydro tile cell coordinate, including the halo, for a world block.
pub(super) fn hydro_cell(block_x: i32, block_z: i32) -> (i32, i32) {
    let local = |block: i32| {
        crate::lattice::floor_div(block, MACRO_CELL)
            - crate::lattice::floor_div(block, crate::description::HYDRO_TILE)
                * crate::hydro::TILE_CELLS
            + crate::hydro::BORDER_CELLS
    };
    (local(block_x), local(block_z))
}
