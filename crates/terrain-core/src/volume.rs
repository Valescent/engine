//! W5 - 3D detail band, caves, entrance mask, and the span scan.
//!
//! `F1.2`, `F1.3`, `F1.5`, `F2.1`, `F2.5`, `F3.1`, `F3.2`, `F3.4`, `F4.1`. This is
//! the only module that changes a column after the surface fill: it edits solidity
//! inside the depth-limited detail band, carves the two cave families, lowers the
//! roof at an entrance, and classifies the solid spans the material pass then
//! dresses.
//!
//! # Order (`F1.5`)
//!
//! `Volume::apply` runs two passes in a fixed order: **detail, then caves**. The
//! cave roof is tested against the highest solid block *after* the detail pass, so
//! an overhang cannot hide a cave mouth, and a cave can never breach what detail
//! left above it.
//!
//! # The lattice (`F1.3`)
//!
//! The density lattice is the global 4x4x4 grid, 96 cells per 384-tall column, with
//! samples at `4k + 0.5` on every axis. `lattice_cell` owns the `y` half of that
//! rule and must agree with `crate::lattice::macro_cell`; a test pins the two
//! together. The lattice is a pure function of absolute world position: it is never
//! anchored to a chunk, a section, or a tile.
//!
//! A lattice cell whose four-block neighbourhood is entirely above
//! `top_solid_y - CAVE_MIN_ROOF`, or entirely below `CAVE_FLOOR_Y` (`y = 8`),
//! is never sampled (`skippable_band`, `cell_skipped`). That is what lets a
//! column stop at the roof instead of walking 96 cells through air above the
//! terrain.
//!
//! **Column path.** The tile driver owns the `x`/`z` half of the lattice because
//! interpolation there needs the four neighbouring cell centres. A single column
//! does not carry them, so this module samples `x` and `z` at the block centre and
//! interpolates the lattice along `y` only. At a lattice point
//! (`4kx + 0.5`, `4k + 0.5`, `4kz + 0.5`) the two paths agree bit for bit; between
//! them this one is strictly finer in `x` and `z`, never coarser.
//!
//! # Cheese and spaghetti (`F2.1`)
//!
//! Exactly `CAVE_TAP_BUDGET` noise taps per 3D sample: two octaves of cheese fBm
//! at `1/48`, and two independent single-octave spaghetti fields at `1/64`. The
//! combined density is `max(cheese, spaghetti)` in `[0, 1]`, and rock becomes air
//! where `density > cave_t` (the `F1.2` threshold). Every sample reports its own tap
//! count, and `VolumeStats::taps` totals a whole column.
//!
//! Both spaghetti fields are `[0, 1]` (`B1.2`), so the interview's `|a|` is the
//! distance from the neutral 0.5, doubled to span `[0, 1]` again. A tunnel pinches
//! where the two fields run nearly parallel; the radius floor is a design target
//! measured on the lower quartile of axis radii, not a promise about every cell.
//!
//! Both families are weighted by the `F2.3` depth windows, which are monotone
//! trapezoids named in this file rather than curves hidden in data. Cheese is
//! `curve_cheese_depth(d) * curve_cheese_floor(y)`: zero below twelve blocks,
//! full by forty, flat to a hundred and forty, zero by two hundred, and zero
//! below `y = 8` / full by `y = 24`. It is also zero on a near-sea column
//! (`top_solid_y <= SEA + 1`) inside twenty-four blocks of its top, so a beach
//! or sea floor stays sealed. Spaghetti is
//! `curve_spag_y(y) * curve_spag_depth(d)`: zero below `y = 12`, full from 24,
//! flat to 296, zero by 308, and zero until four blocks below the top / full by
//! sixteen. A lowland column (`top_solid_y <= SEA + 8`) caps the spaghetti
//! ceiling at `SEA - 8`, so a coastal plain gets no shallow tunnels.
//!
//! The weights are a function of `(top_solid_y, y)`, not a third noise tap, so
//! they cost no budget. The two cheese octaves and the two spaghetti fields are
//! still exactly `CAVE_TAP_BUDGET`.
//!
//! # Entrance (`F2.5`)
//!
//! The entrance field is a two-octave fBm at `1/192` through `Volume::entrance`'s
//! curve. Above `entrance_t` it lowers the roof from `crate::description::CAVE_MIN_ROOF`
//! to zero over a 0.05 ramp and eases the density threshold, which is what turns a
//! chamber into a mouth. Hard exclusions are applied first and are never overridden:
//! `y < 1` is out of range for the carve, `top_solid_y <= SEA + 1` is sea floor,
//! river and lake beds and columns within `BANK_CLEARANCE` blocks of a bank are
//! excluded, and a mouth needs `slope >= MOUTH_SLOPE_MIN`.
//!
//! `Volume::is_mouth` is the coordinate half of the test: the entrance field
//! crossing `entrance_t` at `(x, z)`. `Volume::mouth_open` is the complete gate
//! including slope, height, and the three exclusions, and `Volume::roof` returns
//! the thickness it produces. A flag the caller keeps per column is what lets the
//! span pass treat a mouth column's top as open to the sky even when a one-block
//! lip survives.
//!
//! # Detail band (`F3.1`, `F3.2`)
//!
//! Detail applies only inside `[base_top - detail_down, base_top + detail_up]`, with
//! `base = clamp((h - (y + 0.5)) / 8, -1, 1)` and one 3D tap multiplied by
//! `detail_amp * band_fade(y) * mask`. `band_fade` is zero at both band edges, so
//! the band never leaves a seam, and `mask = curve_detail(roughness) * coast_mask *
//! curve_slope(macro_slope)` fades the band out at the coast and on flat ground.
//!
//! A cell above the height surface (`base <= 0`) only becomes solid when the total
//! density clears `FLOAT_MIN`, so a small noise bump cannot spawn a one-block
//! floater. `F3.4` removes detail entirely over shallow water (`top <= SEA + 2`),
//! on river and lake beds, and within `BANK_CLEARANCE` blocks of a bank.
//!
//! # Spans (`F4.1`)
//!
//! `resolve_spans` groups a finished column into maximal solid runs. One run is a
//! `SpanKind::Surface` column and keeps the `E4.5` stack unchanged. With more than
//! one run the topmost span is the cave's `SpanKind::Ceiling` (its underside is
//! stone), and every span below the first is a `SpanKind::CaveFloor` whose top
//! takes the reduced rule set from `span_rule`: stone and gravel only, no snow, no
//! beach, and no biome topsoil unless `open_to_sky` proves the column is actually
//! open.
//!
//! `open_to_sky` is a skylight walk, not a span count: a shaft that reaches the sky
//! is open even inside a many-span column, and a pocket roofed by one block is not.
//!
//! # Contract notes
//!
//! - `ColumnInput` carries `x` and `z` in addition to the field list in the
//!   brief. The 3D fields are functions of world position; without the column's
//!   coordinates no two columns could differ, so the coordinate half is supplied
//!   like every other fact and is never recomputed here.
//! - `apply` expects the column to hold solid blocks and `air` only, as `F1.5`
//!   step 2 leaves the solid half. Water is placed after the volume pass: a cave
//!   never floods (`D7.1`), and a carve that ran over water would delete it.

//! # Determinism
//!
//! Every operation is `+ - * /`, `min`, `max`, `abs`, `clamp`, or a comparison, in
//! a fixed order. No `sin`, `cos`, `pow`, `exp`, `ln`, no `mul_add`, no hash map,
//! no float reduction of unfixed order, and no value can become NaN: every divisor
//! is a positive constant or guarded, and every curve is clamped by construction.
//! Every loop is bounded by a constant or the column length (`S1.3`).

mod caves;
mod detail;
mod spans;
#[cfg(test)]
mod tests;

#[allow(unused_imports)] // re-exported for the internal stages and unit tests
pub(crate) use caves::{
    BANK_CLEARANCE, CAVE_TAP_BUDGET, CaveSample, MOUTH_SLOPE_MIN, SpanSkip, cell_skipped,
    lattice_cell, skippable_band,
};
#[allow(unused_imports)] // re-exported for the internal stages and unit tests
pub(crate) use detail::{DETAIL_TAP_BUDGET, DetailSample, FLOAT_MIN, band_fade};
#[allow(unused_imports)] // re-exported for the internal stages and unit tests
pub(crate) use spans::{Span, SpanKind, SpanRule, open_to_sky, resolve_spans, span_rule};

use crate::curve::Curve;
use crate::description::{CAVE_MIN_ROOF, DETAIL_DOWN, DETAIL_UP, MACRO_CELL, SEA, WORLD_HEIGHT};
use crate::error::{ErrorKind, Stage, TerrainError};
use crate::hash::{domain, hash64};
use crate::section::BlockId;

use self::caves::ENTRANCE_KNOTS;
use self::detail::{DETAIL_KNOTS, SLOPE_KNOTS};

/// Blocks in one column: `WORLD_HEIGHT` as a `usize` for array lengths.
pub(crate) const COLUMN_HEIGHT: usize = WORLD_HEIGHT as usize;

/// Lattice cells in one 384-tall column: `WORLD_HEIGHT / MACRO_CELL` (`F1.3`).
pub(crate) const LATTICE_CELLS: usize = COLUMN_HEIGHT / MACRO_CELL as usize;

const _: () = assert!(LATTICE_CELLS == 96);

/// World `y` of the last block no pass may write.
#[allow(clippy::cast_possible_wrap)]
const WORLD_LAST_Y: i32 = WORLD_HEIGHT as i32 - 1;

/// Tunable volume parameters. Every field is a required key in the description.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct VolumeParams {
    /// Density above which a cave cell becomes air, in `[0, 1]` (`F1.2`).
    pub cave_t: f32,
    /// Entrance field above which the roof starts to open, in `[0, 1]` (`F2.5`).
    pub entrance_t: f32,
    /// Default roof above a cave, in blocks (`F2.5`).
    pub cave_min_roof: i32,
    /// Detail band depth below the top, in blocks (`F3.3`).
    pub detail_down: i32,
    /// Detail band height above the top, in blocks (`F3.3`).
    pub detail_up: i32,
    /// Amplitude of the detail noise, in density units (`F3.2`).
    pub detail_amp: f32,
    /// Density a floating cell must clear (`F3.2`).
    pub float_min: f32,
    /// Sea level, in blocks (`C1.2`).
    pub sea: i32,
    /// World seed. Keys the cheese, the tunnels, the entrance, and detail.
    pub seed: u64,
}

impl VolumeParams {
    /// The v1 defaults, pinned to the description's own values.
    #[must_use]
    pub(crate) fn v1_default(seed: u64) -> Self {
        Self {
            cave_t: 0.55,
            entrance_t: 0.82,
            cave_min_roof: CAVE_MIN_ROOF,
            detail_down: DETAIL_DOWN,
            detail_up: DETAIL_UP,
            detail_amp: 1.0,
            float_min: FLOAT_MIN,
            sea: SEA,
            seed,
        }
    }

    /// Rejects parameters a world could not evaluate.
    ///
    /// # Errors
    ///
    /// Returns [`Stage::Volume`] with [`ErrorKind::DescriptionInvalid`] naming the
    /// first key that is non-finite or out of range.
    pub(crate) fn validate(&self) -> Result<(), TerrainError> {
        for (key, value) in [
            ("CAVE_T", self.cave_t),
            ("ENTRANCE_T", self.entrance_t),
            ("DETAIL_AMP", self.detail_amp),
            ("FLOAT_MIN", self.float_min),
        ] {
            if !value.is_finite() {
                return Err(reject(key, "finite"));
            }
        }
        if !(0.0..=1.0).contains(&self.cave_t) {
            return Err(reject("CAVE_T", "range"));
        }
        if !(0.0..=1.0).contains(&self.entrance_t) {
            return Err(reject("ENTRANCE_T", "range"));
        }
        if self.cave_min_roof < 0 {
            return Err(reject("CAVE_MIN_ROOF", "non-negative"));
        }
        if self.detail_down < 0 {
            return Err(reject("DETAIL_DOWN", "non-negative"));
        }
        if self.detail_up < 0 {
            return Err(reject("DETAIL_UP", "non-negative"));
        }
        if self.detail_amp < 0.0 {
            return Err(reject("DETAIL_AMP", "non-negative"));
        }
        if !(0.0..=1.0).contains(&self.float_min) {
            return Err(reject("FLOAT_MIN", "range"));
        }
        if self.sea <= 0 || self.sea >= WORLD_LAST_Y {
            return Err(reject("SEA", "range"));
        }
        Ok(())
    }
}

/// Everything the volume pass needs to know about one column (`F1.5`).
///
/// Built by the caller. `x` and `z` are world block coordinates, `top` and
/// `macro_height` come from the height tile, and `rough`/`slope` are the fields the
/// detail mask reads. Nothing here is recomputed by this module.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ColumnInput {
    /// World block `x` of the column.
    pub x: i32,
    /// World block `z` of the column.
    pub z: i32,
    /// Highest solid block of the column, before the detail pass.
    pub top: i32,
    /// Column roughness in `[0, 1]`, the detail curve's input.
    pub rough: f32,
    /// Column integer slope drop (`E4.3`).
    pub slope: i32,
    /// The column is a river channel.
    pub is_river: bool,
    /// The column is a lake bed.
    pub is_lake: bool,
    /// A bank is within [`BANK_CLEARANCE`] blocks.
    pub near_bank: bool,
    /// Pre-floor column height in blocks, the `h` of the detail formula.
    pub macro_height: f32,
}

/// Counters from one [`Volume::apply`] call.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct VolumeStats {
    /// Cells the cave pass turned from solid to air.
    pub cave_cells: u32,
    /// Cells the detail pass changed, in either direction.
    pub detail_cells: u32,
    /// Noise taps the whole column spent.
    pub taps: u32,
    /// `1` when this column opened a mouth, else `0`.
    pub mouths: u32,
}

/// The 3D passes for one world: parameters, compiled curves, and keyed streams.
///
/// Construct once with [`Volume::new`] and reuse it for every column. The curves
/// and the stream ids are pure functions of the parameters and the seed, so the
/// struct is `Send` and gives the same answer from any thread.
#[derive(Clone, Debug)]
pub(crate) struct Volume {
    /// The parameters every pass reads.
    params: VolumeParams,
    /// Raw entrance field to `[0, 1]` (`F2.5`).
    entrance_curve: Curve,
    /// Roughness to detail mask (`F3.2`).
    detail_curve: Curve,
    /// Slope to detail mask (`F3.2`).
    slope_curve: Curve,
    /// Two cheese octave seeds.
    cheese_seeds: [u32; 2],
    /// Two spaghetti field seeds.
    spaghetti_seeds: [u32; 2],
    /// Entrance field seed.
    entrance_seed: u32,
    /// Detail field seed.
    detail_seed: u32,
}

impl Volume {
    /// Compiles the curves and derives the six noise streams.
    ///
    /// # Errors
    ///
    /// Returns [`Stage::Volume`] with [`ErrorKind::DescriptionInvalid`] when the
    /// parameters fail [`VolumeParams::validate`], or when a built-in curve table is
    /// not monotone (which would be a code bug, not a world bug).
    pub(crate) fn new(params: VolumeParams) -> Result<Self, TerrainError> {
        params.validate()?;
        Ok(Self {
            entrance_curve: Curve::new(ENTRANCE_KNOTS)?,
            detail_curve: Curve::new(DETAIL_KNOTS)?,
            slope_curve: Curve::new(SLOPE_KNOTS)?,
            cheese_seeds: [
                stream(params.seed, "cave_cheese_a"),
                stream(params.seed, "cave_cheese_b"),
            ],
            spaghetti_seeds: [
                stream(params.seed, "cave_spaghetti_a"),
                stream(params.seed, "cave_spaghetti_b"),
            ],
            entrance_seed: stream(params.seed, "cave_entrance"),
            detail_seed: stream(params.seed, "detail_band"),
            params,
        })
    }

    /// The parameters this volume was built from.
    #[must_use]
    pub(crate) fn params(&self) -> &VolumeParams {
        &self.params
    }

    /// Carves and dresses one column in place (`F1.5` steps 3 and 4).
    ///
    /// `column` must hold solid blocks up to `input.top` and `air` above, as the
    /// surface fill leaves it. `air` and `stone` are the caller's ids; `stone` is
    /// what the detail pass writes and every cave carve writes `air`.
    ///
    /// The passes run detail first, then caves, and the cave roof is measured
    /// against the highest solid block the detail pass left. `y = 0` and everything
    /// below `y = 1` is never touched (`C4.1`).
    pub(crate) fn apply(
        &self,
        column: &mut [BlockId; WORLD_HEIGHT as usize],
        input: &ColumnInput,
        air: BlockId,
        stone: BlockId,
    ) -> VolumeStats {
        let mut stats = VolumeStats::default();
        self.detail_pass(column, input, air, stone, &mut stats);
        self.cave_pass(column, input, air, &mut stats);
        stats
    }
}

/// A clamped linear ramp from `low` to `high`.
pub(super) fn ramp(low: f32, high: f32, value: f32) -> f32 {
    let span = high - low;
    if span <= 0.0 {
        return if value < low { 0.0 } else { 1.0 };
    }
    ((value - low) / span).clamp(0.0, 1.0)
}

/// Derives one `u32` noise stream from the world seed and a fixed domain tag.
#[allow(clippy::cast_possible_truncation)]
fn stream(seed: u64, name: &str) -> u32 {
    hash64(seed, domain(name), &[]) as u32
}

/// The validation error this module raises.
fn reject(key: &'static str, rule: &'static str) -> TerrainError {
    TerrainError::new(Stage::Volume, ErrorKind::DescriptionInvalid { key, rule })
}
