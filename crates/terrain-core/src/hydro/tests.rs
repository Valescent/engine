//! Tests for the tile grid and the whole-world result.

use super::fixture::{inflow, terrain};
use super::grid::index_of;
use super::*;
use crate::description::MACRO_CELL;

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
                result[index] = inflow(ox + x - HYDRO_BORDER_CELLS, oz + z - HYDRO_BORDER_CELLS);
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
    let neighbour = HydroTile::solve(&grid(TILE_CELLS, 0), &inflow_grid(TILE_CELLS, 0)).unwrap();
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
    let world = WorldHydro::solve(&world_grid(origin, origin, side), side).expect("valid world");

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
