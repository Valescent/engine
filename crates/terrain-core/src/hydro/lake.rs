//! Per-basin lake detection (`D2.6`, `D7.2`).
//!
//! A lake is *not* a filled flat. `D2.6` splits the two: the flood output is
//! routing-only, while a lake is a connected set of cells with real depth, and
//! its water surface is the outlet's filled height, floored once. Interior
//! filled heights never define a surface.
//!
//! # Endorheic basins (`D3.5`)
//!
//! A river may terminate in a closed basin, and v1 allows it. An endorheic lake
//! is recorded exactly like an open one: its surface is the outlet's filled
//! height floored once, its outlet is the lowest cell of the region even when
//! that outlet never reaches the sea, and it is never drained or evaporated in
//! v1. A basin that fails the depth or area gate stays dry, and flow passes
//! through its filled cells so the channel remains continuous.
//!
//! This module does not decide whether a recorded lake is open or closed. The
//! caller follows receivers from [`Lake::outlet`]: a chain that reaches a sea
//! cell is open, one that does not is endorheic.

use super::flow::{D8, neighbour};
use super::{MAX_LAKE_Y, SEA, floor_i32};

/// Minimum water depth for a cell to count as part of a basin: ~1 block.
///
/// A cell shallower than this is a filled flat, not a lake (`D2.6`).
pub(crate) const LAKE_MIN_DEPTH: f32 = 1.0;

/// Minimum area for a recorded lake, in cells (`D7.2`): numbered 12.
pub(crate) const LAKE_MIN_CELLS: usize = 12;

/// Four-connectivity for lake regions: a diagonal touch is not one lake.
const CONNECTED: [D8; 4] = [D8::N, D8::E, D8::S, D8::W];

/// One recorded basin: surface, spill cell, extent and deepest point.
///
/// `cells` is sorted ascending, so the extent is deterministic and diffable.
/// `outlet` is the region cell with the lowest filled height, which is the
/// inner lip of the spill; the surface is `floor(filled[outlet])`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Lake {
    /// Water surface in blocks, never above `super::MAX_LAKE_Y`.
    pub level: i32,
    /// Tile-local cell index of the spill lip, the lowest filled cell.
    pub outlet: u32,
    /// Every cell of the basin, sorted ascending.
    pub cells: Vec<u32>,
    /// Deepest water column, in whole blocks.
    pub max_depth: i32,
}

/// Finds every recorded lake and the per-cell level array (`D2.6`, `D7.2`).
///
/// Returns `(level_of, lakes)`. `level_of` is `0` for dry cells and the surface
/// level for every cell of a recorded lake; because a recorded level is always
/// above [`SEA`], `0` is unambiguous.
///
/// A region is recorded only when all gates pass:
///
/// - it is connected by four-neighbours over cells where
///   `filled > height + LAKE_MIN_DEPTH`,
/// - the surface is above [`SEA`] (otherwise it is just sea),
/// - `max_depth >= 2` blocks,
/// - it covers at least [`LAKE_MIN_CELLS`] cells.
///
/// The surface is `min(floor(filled[outlet]), MAX_LAKE_Y)`. Clamping before the
/// depth gate is what enforces "never above `MAX_LAKE_Y`": a basin whose spill
/// is above the ceiling ends up with no water column and stays dry. The halo
/// pins `filled` to the input height, so a halo cell can never be part of a
/// region and a basin is always contained in the interior.
#[must_use]
pub(crate) fn find_lakes(height: &[f32], filled: &[f32], side: usize) -> (Vec<i32>, Vec<Lake>) {
    let cells = side * side;
    let mut level_of = vec![0_i32; cells];
    let mut lakes: Vec<Lake> = Vec::new();
    if side == 0 {
        return (level_of, lakes);
    }

    let mut visited = vec![false; cells];
    let mut stack: Vec<usize> = Vec::new();
    let mut region: Vec<u32> = Vec::new();

    for seed in 0..cells {
        if visited[seed] {
            continue;
        }
        visited[seed] = true;
        if !wet(height, filled, seed) {
            continue;
        }

        region.clear();
        region.push(u32::try_from(seed).unwrap_or(u32::MAX));
        stack.push(seed);
        for _ in 0..cells {
            let Some(current) = stack.pop() else {
                break;
            };
            for direction in CONNECTED {
                let Some(near) = neighbour(current, side, direction) else {
                    continue;
                };
                if visited[near] {
                    continue;
                }
                visited[near] = true;
                if wet(height, filled, near) {
                    region.push(u32::try_from(near).unwrap_or(u32::MAX));
                    stack.push(near);
                }
            }
        }

        region.sort_unstable();
        let Some(lake) = record(height, filled, &region) else {
            continue;
        };
        for &cell in &lake.cells {
            if let Some(slot) = level_of.get_mut(cell as usize) {
                *slot = lake.level;
            }
        }
        lakes.push(lake);
    }

    lakes.sort_unstable_by_key(|lake| lake.outlet);
    (level_of, lakes)
}

/// Whether one cell holds real water rather than a routing artefact.
fn wet(height: &[f32], filled: &[f32], index: usize) -> bool {
    let (Some(&terrain), Some(&surface)) = (height.get(index), filled.get(index)) else {
        return false;
    };
    surface > terrain + LAKE_MIN_DEPTH
}

/// Applies the gates to one region and builds the record, or `None` if dry.
fn record(height: &[f32], filled: &[f32], region: &[u32]) -> Option<Lake> {
    let first = *region.first()?;
    let mut outlet = first;
    let mut lowest = *filled.get(first as usize)?;
    for &cell in &region[1..] {
        let value = *filled.get(cell as usize)?;
        if value < lowest {
            lowest = value;
            outlet = cell;
        }
    }

    // One floor, at the outlet, and the ceiling applied before the depth gate.
    let level = floor_i32(lowest).min(MAX_LAKE_Y);
    let mut max_depth = i32::MIN;
    for &cell in region {
        let top = floor_i32(*height.get(cell as usize)?);
        max_depth = max_depth.max(level.saturating_sub(top));
    }

    if level <= SEA || max_depth < 2 || region.len() < LAKE_MIN_CELLS {
        return None;
    }

    Some(Lake {
        level,
        outlet,
        cells: region.to_vec(),
        max_depth,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIDE: usize = 16;

    /// A high plateau, well above sea, so the "at or below sea" gate cannot
    /// interfere with the other two.
    const PLATEAU: f32 = 200.0;
    /// Basin floor, five blocks under the plateau.
    const FLOOR: f32 = 195.0;

    fn flat() -> (Vec<f32>, Vec<f32>) {
        (vec![PLATEAU; SIDE * SIDE], vec![PLATEAU; SIDE * SIDE])
    }

    /// Carves a rectangular basin: sunken terrain at [`FLOOR`], flooded to
    /// `surface`.
    fn basin(height: &mut [f32], filled: &mut [f32], x0: usize, z0: usize, size: usize, top: f32) {
        for z in z0..z0 + size {
            for x in x0..x0 + size {
                height[z * SIDE + x] = FLOOR;
                filled[z * SIDE + x] = top;
            }
        }
    }

    #[test]
    fn lakes_respect_both_gates() {
        let (mut height, mut filled) = flat();

        // Passes both gates: 4x4 = 16 cells, three blocks deep.
        basin(&mut height, &mut filled, 1, 1, 4, 198.0);
        // Fails the area gate: 3x3 = 9 cells, deep enough.
        basin(&mut height, &mut filled, 9, 1, 3, 198.0);
        // Fails the depth gate: 4x4, one block deep.
        basin(&mut height, &mut filled, 1, 9, 4, 196.0);
        // Below sea: a coastal basin is sea and is not recorded.
        let sea = super::super::sea_level();
        for z in 9..13 {
            for x in 9..13 {
                height[z * SIDE + x] = 120.0;
                filled[z * SIDE + x] = sea;
            }
        }

        let (level_of, lakes) = find_lakes(&height, &filled, SIDE);

        assert_eq!(lakes.len(), 1, "only the first basin passes both gates");
        let lake = &lakes[0];
        assert_eq!(lake.cells.len(), 16);
        assert_eq!(lake.level, 198);
        assert_eq!(lake.max_depth, 3);
        assert!(lake.cells.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(
            *lake.cells.first().unwrap(),
            u32::try_from(SIDE + 1).unwrap()
        );
        assert_eq!(lake.outlet, *lake.cells.first().unwrap());

        assert_eq!(level_of[SIDE + 1], 198);
        assert_eq!(level_of[SIDE + 10], 0, "the small basin stays dry");
        assert_eq!(level_of[9 * SIDE + 1], 0, "the shallow basin stays dry");
        assert_eq!(level_of[9 * SIDE + 9], 0, "a coastal basin is sea");
        assert_eq!(level_of[0], 0);
    }

    #[test]
    fn lake_level_is_the_outlet_not_the_interior() {
        let (mut height, mut filled) = flat();
        basin(&mut height, &mut filled, 1, 1, 4, 198.0);

        // Interior filled heights rise away from the spill: 198.0 at the lip,
        // 199.5 deep inside. The surface must come from the lip.
        let mut index = 0.0_f32;
        for z in 1..5 {
            for x in 1..5 {
                filled[z * SIDE + x] = 198.0 + index * 0.1;
                index += 1.0;
            }
        }

        let (_, lakes) = find_lakes(&height, &filled, SIDE);
        assert_eq!(lakes.len(), 1);
        let lake = &lakes[0];

        assert_eq!(
            *lake.cells.first().unwrap(),
            u32::try_from(SIDE + 1).unwrap()
        );
        assert_eq!(lake.outlet, u32::try_from(SIDE + 1).unwrap());
        assert_eq!(lake.level, 198, "the outlet floors to 198");
        assert_eq!(
            floor_i32(filled[4 * SIDE + 4]),
            199,
            "the interior floor is 199"
        );
        assert_ne!(lake.level, floor_i32(filled[4 * SIDE + 4]));
    }

    #[test]
    fn a_closed_bowl_ends_in_a_lake_at_its_spill_height() {
        let (mut height, mut filled) = flat();
        // A closed bowl on a plateau, with no sea anywhere in the grid. `D3.5`
        // keeps the lake: it is endorheic, not a failed basin.
        basin(&mut height, &mut filled, 1, 1, 4, PLATEAU);

        assert!(
            height
                .iter()
                .all(|value| *value > super::super::sea_level()),
            "the fixture must have no sea, so the basin cannot drain to one"
        );

        let (level_of, lakes) = find_lakes(&height, &filled, SIDE);
        assert_eq!(lakes.len(), 1, "an endorheic bowl is still a lake");

        let lake = &lakes[0];
        assert_eq!(lake.level, floor_i32(PLATEAU), "spill height, floored once");
        assert_eq!(lake.max_depth, 5, "the bowl floor is 195");
        assert_eq!(
            floor_i32(filled[lake.outlet as usize]),
            lake.level,
            "the surface comes from the outlet"
        );
        assert!(
            lakes[0]
                .cells
                .iter()
                .all(|&cell| level_of[cell as usize] == lake.level),
            "every bowl cell carries the same closed-lake level"
        );
    }

    #[test]
    fn a_basin_above_the_lake_ceiling_stays_dry() {
        let (mut height, mut filled) = flat();
        // Four blocks of water, but the surface is above MAX_LAKE_Y.
        let ceiling = f32::from(u16::try_from(MAX_LAKE_Y).unwrap()) + 4.0;
        for z in 1..5 {
            for x in 1..5 {
                height[z * SIDE + x] = ceiling - 4.0;
                filled[z * SIDE + x] = ceiling;
            }
        }

        let (_, lakes) = find_lakes(&height, &filled, SIDE);
        assert!(lakes.is_empty(), "clamped below the terrain, so no column");
    }

    #[test]
    fn diagonal_touch_does_not_join_two_lakes() {
        let (mut height, mut filled) = flat();
        // Two 4x4 basins that meet only at the corner (4,4)/(5,5): 16 cells
        // each, so both pass the area gate, but 4-connectivity keeps them
        // apart.
        for z in 1..5 {
            for x in 1..5 {
                height[z * SIDE + x] = FLOOR;
                filled[z * SIDE + x] = 198.0;
            }
        }
        for z in 5..9 {
            for x in 5..9 {
                height[z * SIDE + x] = FLOOR;
                filled[z * SIDE + x] = 198.0;
            }
        }

        let (_, lakes) = find_lakes(&height, &filled, SIDE);
        assert_eq!(lakes.len(), 2);
        assert_eq!(lakes[0].cells.len(), 16);
        assert_eq!(lakes[1].cells.len(), 16);
    }
}
