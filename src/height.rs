//! W4 - the derived `HeightTile` and the macro height pipeline.
//!
//! `E4.6`, `C3.1`, `C3.3`, `F1.5`. A `HeightTile` is the only height array the
//! rest of the generator reads: slope rules, tree placement, surface stacking
//! and the volume fill all take their column top from it. It is a pure function
//! of the field tile plus the hydrology tile, disposable, and never hashed.
//!
//! # Grid layout
//!
//! A tile owns `TILE_BLOCKS` blocks on each axis and carries a one-block margin
//! on every side, so [`SIDE`] is `130` and `top` accepts `-1..=128`
//! (`B4.2`). The margin is not a clamped copy of the interior: it is built from
//! the same global lattice and the same per-block fields as the neighbouring
//! tile's interior, so two tiles agree bit-for-bit on the column they share.
//!
//! Block coordinates here are tile-local throughout, including the margin.
//! [`MacroGrid`] therefore needs no origin: sample `i` of its 35-point edge is
//! always lattice cell `i - 1` of this tile, and the caller fills it from the
//! global lattice around its own origin.
//!
//! # Order of the chain
//!
//! Fixed by `F1.5`: bilinear macro height, per-block detail, terrace, river
//! carve, and **one** `floor` at the end. [`top_solid_y`] is the only quantiser
//! in this module (`C3.1`); [`build`] calls it exactly once per column. The
//! terrace grid holds a displacement in blocks, so the carve that follows it is
//! never re-terraced and the risers stay where the graph put them.
//!
//! # The river carve
//!
//! The carve subtracts `influence * depth` from the `f32` height, with
//! `influence` from [`river_influence`] and `depth` from [`bed_depth`] (`D3.3`).
//! A carved bed is then bounded by [`clamp_bed`], which keeps it at or above
//! `SEA - 30` and `y = 1`. The bound never lifts a column: a column already
//! below the bed floor is not carved at all, because the open ocean floor is
//! deliberately lower than the bed floor and is not a river.
//!
//! Lake levels and river water surfaces are hydrology outputs and are not part
//! of the solid chain; the caller applies them when it fills water (`F1.5` step
//! 2). This module only answers where the top solid block is.
//!
//! # Determinism
//!
//! Every operation is `+ - * /`, `min`, `max`, `floor`, or a comparison, in a
//! fixed order. No `sin`, `cos`, `pow`, `exp`, `ln`, no `mul_add`, no hash map,
//! and no NaN: non-finite grids are rejected at construction rather than
//! clamped (`A2.3`, `S1.4`).

use crate::description::{MAX_TERRAIN_Y, SEA, TILE_BLOCKS};
use crate::error::{ErrorKind, Stage, TerrainError};
use crate::hydro::flow::{bed_depth, clamp_bed, river_influence};
use crate::lattice::{bilinear, macro_cell};

/// Blocks along one edge of a tile, margin included: `128 + 2` (`E4.6`).
pub const SIDE: i32 = 130;

/// Columns in one grid: [`SIDE`] squared.
pub const CELLS: usize = 16_900;

/// Lattice samples along one edge of a [`MacroGrid`]: `32 + 3`, margin included.
pub const MACRO_SIDE: i32 = 35;

/// Lattice samples in one [`MacroGrid`]: [`MACRO_SIDE`] squared.
pub const MACRO_CELLS: usize = 1_225;

/// Row stride of a [`MacroGrid`].
const MACRO_STRIDE: usize = 35;

const _: () = assert!(SIDE == TILE_BLOCKS + 2);
const _: () = assert!(CELLS == 16_900 && MACRO_SIDE == 35);
const _: () = assert!(MACRO_CELLS == MACRO_STRIDE * MACRO_STRIDE);

/// Lowest a carved bed may reach (`D3.3`), mirroring [`clamp_bed`].
///
/// Kept here as a mirror of the hydrology constant rather than read from it,
/// because `height` only needs the value for one comparison. A test pins the two
/// together so they cannot drift.
const BED_FLOOR_Y: i32 = SEA - 30;

/// Maps a tile-local block coordinate to its slot in a [`SIDE`]-square grid.
///
/// This is `(x + 1) + SIDE * (z + 1)`, so `(-1, -1)` is slot `0` and `(128, 128)`
/// is the last slot.
///
/// # Panics
///
/// Panics (by out-of-bounds indexing) when `x` or `z` is outside `-1..=128`.
#[allow(clippy::cast_sign_loss)]
#[must_use]
pub fn index(x: i32, z: i32) -> usize {
    ((x + 1) + SIDE * (z + 1)) as usize
}

/// The top solid block of a height: `floor(h)` (`C3.1`).
///
/// This is the only quantiser in the module and the only place a height becomes
/// an integer. It is applied once per column, after the carve. The cast to
/// `i32` saturates in Rust, so an absurd height clamps instead of wrapping; the
/// graph proof, not this function, keeps heights inside the world (`C1.5`).
#[allow(clippy::cast_possible_truncation)]
#[must_use]
pub fn top_solid_y(h: f32) -> i32 {
    h.floor() as i32
}

/// The derived 130 x 130 tops of one field tile (`E4.6`).
///
/// Disposable: it is rebuilt from the field and hydrology tiles whenever it is
/// missing and is never written to a save or hashed.
#[derive(Clone, Debug)]
pub struct HeightTile {
    tops: Box<[i32]>,
}

impl HeightTile {
    /// The top solid block of one column, margin included.
    ///
    /// # Panics
    ///
    /// Panics when `x` or `z` is outside `-1..=128`.
    #[must_use]
    pub fn top(&self, x: i32, z: i32) -> i32 {
        self.tops[index(x, z)]
    }

    /// The largest one-block step from this column to its four neighbours
    /// (`E4.3`).
    ///
    /// `max(|top - top(x±1)|, |top - top(z±1)|)`, read from this tile only, never
    /// from a neighbouring chunk. Owned columns always have all four neighbours
    /// because of the margin.
    ///
    /// # Panics
    ///
    /// Panics when a neighbour would fall outside `-1..=128`, so callers stay on
    /// the owned `0..=127` range.
    #[must_use]
    pub fn slope_drop(&self, x: i32, z: i32) -> i32 {
        let centre = self.top(x, z);
        let along_x = (centre - self.top(x - 1, z))
            .abs()
            .max((centre - self.top(x + 1, z)).abs());
        let along_z = (centre - self.top(x, z - 1))
            .abs()
            .max((centre - self.top(x, z + 1)).abs());
        along_x.max(along_z)
    }
}

/// The 35 x 35 macro-lattice samples around one tile (`B4.2`, `C3.3`).
///
/// Sample `(i, j)` is lattice cell `(i - 1, j - 1)` of this tile, so the struct
/// is anchored at the tile's own origin without storing it. The caller reads the
/// global lattice for cells `-1..=33` on both axes; a tile one tile east reads
/// cells `31..=65`, and the two agree where they overlap.
#[derive(Clone, Debug)]
pub struct MacroGrid {
    samples: Box<[f32]>,
}

impl MacroGrid {
    /// Wraps 35 x 35 lattice samples, row-major with `z` as the slow axis.
    #[must_use]
    pub fn from_samples(samples: &[f32; MACRO_CELLS]) -> Self {
        Self {
            samples: samples.to_vec().into_boxed_slice(),
        }
    }

    /// Bilinearly interpolates the lattice to one block (`B4.1`, `C3.3`).
    ///
    /// Weights are the exact quarters of [`macro_cell`], so the result is exact
    /// for quarter-representable corners. The block coordinate is tile-local:
    /// `-1..=128`, the same convention as [`index`].
    ///
    /// # Panics
    ///
    /// Panics when `block_x` or `block_z` is outside `-1..=128`.
    #[must_use]
    pub fn sample(&self, block_x: i32, block_z: i32) -> f32 {
        let (cell_x, weight_x) = macro_cell(block_x);
        let (cell_z, weight_z) = macro_cell(block_z);
        // The stored grid starts one cell before the tile's first cell.
        let x = cell_x + 1;
        let z = cell_z + 1;
        bilinear(
            self.at(x, z),
            self.at(x + 1, z),
            self.at(x, z + 1),
            self.at(x + 1, z + 1),
            weight_x,
            weight_z,
        )
    }

    /// One lattice sample by grid index, `0..=34`.
    fn at(&self, x: i32, z: i32) -> f32 {
        debug_assert!(
            (0..MACRO_SIDE).contains(&x) && (0..MACRO_SIDE).contains(&z),
            "macro sample {x},{z} is outside the stored border"
        );
        self.samples[macro_index(x, z)]
    }
}

/// Maps a macro-grid index pair to its slot.
#[allow(clippy::cast_sign_loss)]
fn macro_index(x: i32, z: i32) -> usize {
    x as usize + MACRO_STRIDE * z as usize
}

/// A 130 x 130 per-block field, margin included.
///
/// Detail, terrace displacement, and the two river fields all use this shape.
/// Construction rejects a wrong length and a non-finite value; nothing is
/// clamped (`S1.4`).
#[derive(Clone, Debug)]
pub struct BlockGrid {
    values: Box<[f32]>,
}

impl BlockGrid {
    /// Validates and wraps one 130 x 130 field.
    ///
    /// # Errors
    ///
    /// Returns [`Stage::Height`] with [`ErrorKind::InvariantViolated`] when the
    /// slice is not [`CELLS`] long or holds a NaN or infinity.
    pub fn new(values: Box<[f32]>) -> Result<Self, TerrainError> {
        if values.len() != CELLS {
            return Err(invalid("height grid length"));
        }
        if !values.iter().all(|value| value.is_finite()) {
            return Err(invalid("height grid finite"));
        }
        Ok(Self { values })
    }

    /// The field value at one tile-local column.
    ///
    /// # Panics
    ///
    /// Panics when `x` or `z` is outside `-1..=128`.
    #[must_use]
    pub fn get(&self, x: i32, z: i32) -> f32 {
        self.values[index(x, z)]
    }
}

/// The river fields of one tile, per block (`D3.3`).
///
/// The hydrology tile publishes channel width and lake levels on its own grid;
/// the caller interpolates them to blocks and hands the result over here. The
/// depth grid is derived once by [`RiverCarve::new`] with [`bed_depth`], so the
/// carve and the water level share one curve.
///
/// The distance grid is the block's distance to the channel centre, taken from
/// the interpolated river-influence field, never from a per-block path search
/// (`D3.3`).
#[derive(Clone, Debug)]
pub struct RiverCarve {
    /// Channel width in blocks; `0` where there is no river.
    pub width: BlockGrid,
    /// U-profile bed depth in blocks, from [`bed_depth`].
    pub bed_depth: BlockGrid,
    /// Distance to the channel centre in blocks.
    pub distance: BlockGrid,
}

impl RiverCarve {
    /// Builds a carve from the interpolated width and distance fields.
    ///
    /// The depth grid is `bed_depth(width)` per block: one curve for the whole
    /// tile, so a caller cannot carve with a different profile than the one the
    /// water level assumes.
    #[must_use]
    pub fn new(width: BlockGrid, distance: BlockGrid) -> Self {
        let depths: Vec<f32> = width.values.iter().map(|&value| bed_depth(value)).collect();
        Self {
            width,
            bed_depth: BlockGrid {
                values: depths.into_boxed_slice(),
            },
            distance,
        }
    }

    /// Whether this column is inside the channel at all.
    fn is_river(&self, x: i32, z: i32) -> bool {
        self.width.get(x, z) > 0.0
    }

    /// How many blocks the carve removes at one column.
    fn carve_at(&self, x: i32, z: i32) -> f32 {
        river_influence(self.width.get(x, z), self.distance.get(x, z)) * self.bed_depth.get(x, z)
    }
}

/// Builds the derived tops of one tile (`F1.5`).
///
/// The chain is fixed and applied per column: bilinear macro height, per-block
/// detail, terrace displacement, river carve, one `floor`. `detail` and
/// `terrace` are displacements in blocks, so the sum is
/// `(macro + detail) + terrace` in that order.
///
/// A carve applies only where the column is inside a channel and its terraced
/// height is at or above the bed floor, so the open ocean floor (`SEA - 32`) is
/// never carved and a river never digs below `SEA - 30`.
#[must_use]
pub fn build(
    macro_grid: &MacroGrid,
    detail: &BlockGrid,
    terrace: &BlockGrid,
    rivers: &RiverCarve,
) -> HeightTile {
    let mut tops = vec![0_i32; CELLS].into_boxed_slice();
    for z in -1..=TILE_BLOCKS {
        for x in -1..=TILE_BLOCKS {
            let mut height = macro_grid.sample(x, z);
            height += detail.get(x, z);
            height += terrace.get(x, z);
            let top = if rivers.is_river(x, z) && height >= bed_floor() {
                clamp_bed(top_solid_y(height - rivers.carve_at(x, z)))
            } else {
                top_solid_y(height)
            };
            debug_assert!(
                (1..=MAX_TERRAIN_Y).contains(&top),
                "column {x},{z} left the terrain range at {top}"
            );
            tops[index(x, z)] = top;
        }
    }
    HeightTile { tops }
}

/// [`BED_FLOOR_Y`] as the `f32` the carve compares against.
#[allow(clippy::cast_precision_loss)]
fn bed_floor() -> f32 {
    BED_FLOOR_Y as f32
}

/// The error this module raises for a grid that cannot be used.
fn invalid(check: &'static str) -> TerrainError {
    TerrainError::new(
        Stage::Height,
        ErrorKind::InvariantViolated { check, coord: None },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compares two `f32` bit patterns, so a test never relies on float equality.
    fn assert_bits(actual: f32, expected: f32) {
        assert_eq!(
            actual.to_bits(),
            expected.to_bits(),
            "{actual} and {expected} differ"
        );
    }

    /// Converts an integer fixture value. Never used on the output path.
    #[allow(clippy::cast_precision_loss)]
    fn float(value: i32) -> f32 {
        value as f32
    }

    fn grid(value: f32) -> BlockGrid {
        BlockGrid::new(vec![value; CELLS].into_boxed_slice()).expect("a flat grid is valid")
    }

    fn grid_from(field: impl Fn(i32, i32) -> f32) -> BlockGrid {
        let mut values = vec![0.0_f32; CELLS];
        for z in -1..=TILE_BLOCKS {
            for x in -1..=TILE_BLOCKS {
                values[index(x, z)] = field(x, z);
            }
        }
        BlockGrid::new(values.into_boxed_slice()).expect("the fixture grid is valid")
    }

    fn flat_macro(value: f32) -> MacroGrid {
        MacroGrid::from_samples(&[value; MACRO_CELLS])
    }

    /// A lattice whose value is `4 * cx + 8 * cz` at cell `(cx, cz)`.
    #[allow(clippy::cast_precision_loss)]
    fn linear_macro() -> MacroGrid {
        let mut samples = [0.0_f32; MACRO_CELLS];
        for cz in -1..=33 {
            for cx in -1..=33 {
                samples[macro_index(cx + 1, cz + 1)] = 4.0 * cx as f32 + 8.0 * cz as f32;
            }
        }
        MacroGrid::from_samples(&samples)
    }

    /// The lattice of the `(0, 0)` and `(1, 0)` fixtures: one global function of
    /// the world cell.
    #[allow(clippy::cast_precision_loss)]
    fn seam_lattice(cx: i32, cz: i32) -> f32 {
        128.0 + 0.25 * cx as f32 - 0.5 * cz as f32
    }

    /// A macro grid for a tile whose origin is `(origin_x, origin_z)` blocks.
    #[allow(clippy::cast_precision_loss)]
    fn macro_for(origin_x: i32, origin_z: i32) -> MacroGrid {
        let mut samples = [0.0_f32; MACRO_CELLS];
        for cz in -1..=33 {
            for cx in -1..=33 {
                // Grid index `i` is the tile's local cell `i - 1`, which is the
                // world cell `origin / 4 - 1 + i`.
                samples[macro_index(cx + 1, cz + 1)] =
                    seam_lattice(origin_x / 4 + cx, origin_z / 4 + cz);
            }
        }
        MacroGrid::from_samples(&samples)
    }

    /// A carve with no river anywhere.
    fn no_river() -> RiverCarve {
        RiverCarve::new(grid(0.0), grid(0.0))
    }

    /// A carve with one channel width and one distance per block.
    fn carve(width: f32, distance: f32) -> RiverCarve {
        RiverCarve::new(grid(width), grid(distance))
    }

    #[test]
    fn top_solid_y_is_floor() {
        assert_eq!(top_solid_y(72.4), 72);
        assert_eq!(top_solid_y(-0.1), -1);
        assert_eq!(top_solid_y(0.0), 0);
        assert_eq!(top_solid_y(127.999_99), 127);
        assert_eq!(top_solid_y(-72.4), -73);
    }

    #[test]
    fn bilinear_uses_exact_quarters() {
        let macro_grid = linear_macro();

        // Cell centres: weight zero on every axis.
        assert_bits(macro_grid.sample(0, 0), 0.0);
        assert_bits(macro_grid.sample(4, 0), 4.0);
        assert_bits(macro_grid.sample(0, 4), 8.0);
        assert_bits(macro_grid.sample(-4, 0), -4.0);

        // Midpoints and quarter points are exact in `f32`.
        assert_bits(macro_grid.sample(1, 0), 1.0);
        assert_bits(macro_grid.sample(2, 0), 2.0);
        assert_bits(macro_grid.sample(3, 0), 3.0);
        assert_bits(macro_grid.sample(0, 2), 4.0);
        assert_bits(macro_grid.sample(2, 2), 6.0);
        assert_bits(macro_grid.sample(-2, 0), -2.0);
        assert_bits(macro_grid.sample(-1, 0), -1.0);

        // The margin is interpolated from the same lattice, not clamped.
        assert_bits(macro_grid.sample(128, 0), 128.0);
        assert_bits(macro_grid.sample(-1, -1), -3.0);
    }

    #[test]
    fn pipeline_order_is_fixed() {
        // A plain with a 0.1 terrace displacement and a river whose carve sweeps
        // to 0.6753273 blocks. With these floats, terracing first lands the
        // column exactly on 100 and carving first lands it an ulp below, so the
        // two orders floor to different tops. The fixture was found by sweeping
        // the carve; it pins the order, not the arithmetic.
        let macro_grid = flat_macro(100.575_325);
        let detail = grid(0.0);
        let terrace = grid(0.1);
        let rivers = carve(2.0, 1.609_998_8);

        let height = 100.575_325_f32 + 0.1_f32;
        let removed = rivers.carve_at(0, 0);
        let documented = top_solid_y(height - removed);
        let alternative = top_solid_y((100.575_325_f32 - removed) + 0.1_f32);
        assert_eq!(documented, 100, "the fixture must be a boundary case");
        assert_ne!(documented, alternative, "the two orders must differ");

        let tile = build(&macro_grid, &detail, &terrace, &rivers);

        // The documented order: the terrace lands first, the carve is subtracted
        // from the terraced height, and only then is the column floored.
        for (x, z) in [(0, 0), (63, 91), (-1, -1), (128, 128)] {
            assert_eq!(tile.top(x, z), documented, "{x},{z}");
        }
    }

    #[test]
    #[allow(clippy::cast_possible_truncation)]
    fn one_quantiser_only() {
        // The same float sum, reached through two different grid paths: one puts
        // it in the macro sample, the other splits it between detail and terrace.
        let first = build(&flat_macro(100.5), &grid(0.0), &grid(0.0), &no_river());
        let second = build(&flat_macro(100.0), &grid(0.5), &grid(0.0), &no_river());
        let third = build(&flat_macro(100.0), &grid(0.25), &grid(0.25), &no_river());

        for (x, z) in [(0, 0), (37, 91), (-1, 64), (128, 128)] {
            assert_eq!(first.top(x, z), 100, "{x},{z}");
            assert_eq!(second.top(x, z), 100, "{x},{z}");
            assert_eq!(third.top(x, z), 100, "{x},{z}");
        }

        // `top == floor(h)` for a height that is not an integer.
        let tile = build(&flat_macro(72.4), &grid(0.0), &grid(0.0), &no_river());
        assert_eq!(tile.top(0, 0), 72.4_f32.floor() as i32);
        assert_eq!(tile.top(0, 0), 72);

        // And the source has no other quantiser: one `floor`, one integer cast,
        // no `ceil` and no `round` outside this test module.
        let source = include_str!("height.rs");
        let (production, _) = source
            .split_once("#[cfg(test)]")
            .expect("the test module marker exists");
        assert_eq!(production.matches(".floor()").count(), 1);
        assert_eq!(production.matches(" as i32").count(), 1);
        assert!(!production.contains(".ceil()"));
        assert!(!production.contains(".round()"));
    }

    #[test]
    fn margin_matches_the_interior() {
        let west = build(&macro_for(0, 0), &grid(0.0), &grid(0.0), &no_river());
        let east = build(
            &macro_for(TILE_BLOCKS, 0),
            &grid(0.0),
            &grid(0.0),
            &no_river(),
        );
        let north = build(&macro_for(0, 0), &grid(0.0), &grid(0.0), &no_river());
        let south = build(
            &macro_for(0, TILE_BLOCKS),
            &grid(0.0),
            &grid(0.0),
            &no_river(),
        );

        // The shared column is world block 128: the west tile's right margin and
        // the east tile's left margin are the same global lattice point.
        for z in -1..=TILE_BLOCKS {
            assert_eq!(west.top(TILE_BLOCKS, z), east.top(0, z), "x seam at z {z}");
        }
        // The margin is also the neighbour's interior: block 127 is interior in
        // the west tile and margin in the east.
        for z in 0..TILE_BLOCKS {
            assert_eq!(
                west.top(TILE_BLOCKS - 1, z),
                east.top(-1, z),
                "x neighbour at z {z}"
            );
        }

        // And the same across the `z` seam.
        for x in -1..=TILE_BLOCKS {
            assert_eq!(
                north.top(x, TILE_BLOCKS),
                south.top(x, 0),
                "z seam at x {x}"
            );
        }
        for x in 0..TILE_BLOCKS {
            assert_eq!(
                north.top(x, TILE_BLOCKS - 1),
                south.top(x, -1),
                "z neighbour at x {x}"
            );
        }

        // The margin is interpolated, not copied: it is not flat along the seam.
        assert_ne!(west.top(TILE_BLOCKS, 0), west.top(TILE_BLOCKS, 8));
    }

    #[test]
    fn slope_drop_reads_the_tile() {
        // Heights come straight from the detail grid, so every top is exact.
        let ramp = grid_from(|x, _| float(x + 2));
        let tile = build(&flat_macro(0.0), &ramp, &grid(0.0), &no_river());
        for x in 0..TILE_BLOCKS {
            assert_eq!(tile.slope_drop(x, 40), 1, "ramp at {x}");
        }
        assert_eq!(tile.slope_drop(0, 40), 1, "the margin continues the ramp");

        let cliff = grid_from(|x, _| if x >= 64 { 105.0 } else { 100.0 });
        let tile = build(&flat_macro(0.0), &cliff, &grid(0.0), &no_river());
        assert_eq!(tile.slope_drop(63, 40), 5);
        assert_eq!(tile.slope_drop(64, 40), 5);
        assert_eq!(tile.slope_drop(30, 40), 0);

        let plateau = build(&flat_macro(0.0), &grid(50.0), &grid(0.0), &no_river());
        for (x, z) in [(0, 0), (127, 127), (64, 12)] {
            assert_eq!(plateau.slope_drop(x, z), 0, "{x},{z}");
        }
    }

    #[test]
    fn river_bed_is_clamped() {
        // `SEA - 30` is 98, so the `y = 1` bound never binds.
        assert_eq!(BED_FLOOR_Y, SEA - 30);
        assert_eq!(clamp_bed(i32::MIN), BED_FLOOR_Y);

        // A six-block carve from 120 stops well above the floor.
        let deep = carve(12.0, 0.0);
        let tile = build(&flat_macro(120.0), &grid(0.0), &grid(0.0), &deep);
        assert_eq!(tile.top(0, 0), 114);

        // From 100 the same carve would reach 94; the floor holds it at 98.
        let tile = build(&flat_macro(100.0), &grid(0.0), &grid(0.0), &deep);
        assert_eq!(tile.top(0, 0), BED_FLOOR_Y);

        // The ocean floor is not a river bed: a channel there cannot lift it,
        // and it cannot dig below the floor either.
        let shelf = build(&flat_macro(96.0), &grid(0.0), &grid(0.0), &deep);
        assert_eq!(shelf.top(0, 0), 96);

        // An absurd depth grid still cannot break the floor.
        let mut wild = carve(12.0, 0.0);
        wild.bed_depth = grid(200.0);
        let tile = build(&flat_macro(120.0), &grid(0.0), &grid(0.0), &wild);
        assert_eq!(tile.top(0, 0), BED_FLOOR_Y);
        assert!(tile.top(0, 0) >= 1);

        // No channel means no carve at all, even with a deep depth grid.
        let mut dry = carve(0.0, 0.0);
        dry.bed_depth = grid(200.0);
        let dry = build(&flat_macro(120.0), &grid(0.0), &grid(0.0), &dry);
        assert_eq!(dry.top(0, 0), 120);
    }

    #[test]
    fn grids_reject_bad_values() {
        let short = BlockGrid::new(vec![0.0_f32; CELLS - 1].into_boxed_slice());
        let error = short.expect_err("a short grid is rejected");
        assert_eq!(error.stage, Stage::Height);
        assert!(matches!(error.kind, ErrorKind::InvariantViolated { .. }));

        let mut values = vec![0.0_f32; CELLS];
        values[17] = f32::NAN;
        let nan = BlockGrid::new(values.into_boxed_slice());
        assert!(nan.is_err(), "a non-finite grid is rejected, not clamped");
    }
}
