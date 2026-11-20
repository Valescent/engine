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
//! [`Volume::apply`] runs two passes in a fixed order: **detail, then caves**. The
//! cave roof is tested against the highest solid block *after* the detail pass, so
//! an overhang cannot hide a cave mouth, and a cave can never breach what detail
//! left above it.
//!
//! # The lattice (`F1.3`)
//!
//! The density lattice is the global 4x4x4 grid, 96 cells per 384-tall column, with
//! samples at `4k + 0.5` on every axis. [`lattice_cell`] owns the `y` half of that
//! rule and must agree with [`crate::lattice::macro_cell`]; a test pins the two
//! together. The lattice is a pure function of absolute world position: it is never
//! anchored to a chunk, a section, or a tile.
//!
//! A lattice cell whose four-block neighbourhood is entirely above
//! `top_solid_y - CAVE_MIN_ROOF`, or entirely below `CAVE_FLOOR_Y` (`y = 8`),
//! is never sampled ([`skippable_band`], [`cell_skipped`]). That is what lets a
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
//! Exactly [`CAVE_TAP_BUDGET`] noise taps per 3D sample: two octaves of cheese fBm
//! at `1/48`, and two independent single-octave spaghetti fields at `1/64`. The
//! combined density is `max(cheese, spaghetti)` in `[0, 1]`, and rock becomes air
//! where `density > cave_t` (the `F1.2` threshold). Every sample reports its own tap
//! count, and [`VolumeStats::taps`] totals a whole column.
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
//! still exactly [`CAVE_TAP_BUDGET`].
//!
//! # Entrance (`F2.5`)
//!
//! The entrance field is a two-octave fBm at `1/192` through [`Volume::entrance`]'s
//! curve. Above `entrance_t` it lowers the roof from [`crate::description::CAVE_MIN_ROOF`]
//! to zero over a 0.05 ramp and eases the density threshold, which is what turns a
//! chamber into a mouth. Hard exclusions are applied first and are never overridden:
//! `y < 1` is out of range for the carve, `top_solid_y <= SEA + 1` is sea floor,
//! river and lake beds and columns within [`BANK_CLEARANCE`] blocks of a bank are
//! excluded, and a mouth needs `slope >= MOUTH_SLOPE_MIN`.
//!
//! [`Volume::is_mouth`] is the coordinate half of the test: the entrance field
//! crossing `entrance_t` at `(x, z)`. [`Volume::mouth_open`] is the complete gate
//! including slope, height, and the three exclusions, and [`Volume::roof`] returns
//! the thickness it produces. A flag the caller keeps per column is what lets the
//! span pass treat a mouth column's top as open to the sky even when a one-block
//! lip survives.
//!
//! # Detail band (`F3.1`, `F3.2`)
//!
//! Detail applies only inside `[base_top - detail_down, base_top + detail_up]`, with
//! `base = clamp((h - (y + 0.5)) / 8, -1, 1)` and one 3D tap multiplied by
//! `detail_amp * band_fade(y) * mask`. [`band_fade`] is zero at both band edges, so
//! the band never leaves a seam, and `mask = curve_detail(roughness) * coast_mask *
//! curve_slope(macro_slope)` fades the band out at the coast and on flat ground.
//!
//! A cell above the height surface (`base <= 0`) only becomes solid when the total
//! density clears [`FLOAT_MIN`], so a small noise bump cannot spawn a one-block
//! floater. `F3.4` removes detail entirely over shallow water (`top <= SEA + 2`),
//! on river and lake beds, and within [`BANK_CLEARANCE`] blocks of a bank.
//!
//! # Spans (`F4.1`)
//!
//! [`resolve_spans`] groups a finished column into maximal solid runs. One run is a
//! [`SpanKind::Surface`] column and keeps the `E4.5` stack unchanged. With more than
//! one run the topmost span is the cave's [`SpanKind::Ceiling`] (its underside is
//! stone), and every span below the first is a [`SpanKind::CaveFloor`] whose top
//! takes the reduced rule set from [`span_rule`]: stone and gravel only, no snow, no
//! beach, and no biome topsoil unless [`open_to_sky`] proves the column is actually
//! open.
//!
//! `open_to_sky` is a skylight walk, not a span count: a shaft that reaches the sky
//! is open even inside a many-span column, and a pocket roofed by one block is not.
//!
//! # Contract notes
//!
//! - [`ColumnInput`] carries `x` and `z` in addition to the field list in the
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

use crate::curve::Curve;
use crate::description::{CAVE_MIN_ROOF, DETAIL_DOWN, DETAIL_UP, MACRO_CELL, SEA, WORLD_HEIGHT};
use crate::error::{ErrorKind, Stage, TerrainError};
use crate::hash::{domain, hash64};
use crate::noise::{fbm_2d, simplex2_3d, unit};
use crate::section::BlockId;

/// Noise taps a single 3D cave sample may spend (`F2.1`).
///
/// Two cheese octaves plus two spaghetti fields. The counter in [`CaveSample`] is
/// the assertion: a sample that spent more would be a version bump.
pub const CAVE_TAP_BUDGET: u8 = 4;

/// Noise taps a single detail sample may spend (`F3.2`).
///
/// One 3D tap today; the budget is the ceiling the band is allowed to grow into.
pub const DETAIL_TAP_BUDGET: u8 = 4;

/// Integer slope drop at which a column may open a cave mouth (`F2.5`).
pub const MOUTH_SLOPE_MIN: i32 = 2;

/// Clearance, in blocks, that keeps a mouth off a river or lake bank (`F2.5`).
pub const BANK_CLEARANCE: i32 = 2;

/// Density a floating cell must clear to become solid (`F3.2`).
pub const FLOAT_MIN: f32 = 0.15;

/// Blocks in one column: `WORLD_HEIGHT` as a `usize` for array lengths.
pub const COLUMN_HEIGHT: usize = WORLD_HEIGHT as usize;

/// Lattice cells in one 384-tall column: `WORLD_HEIGHT / MACRO_CELL` (`F1.3`).
pub const LATTICE_CELLS: usize = COLUMN_HEIGHT / MACRO_CELL as usize;

const _: () = assert!(LATTICE_CELLS == 96);

/// World `y` of the last block no pass may write.
#[allow(clippy::cast_possible_wrap)]
const WORLD_LAST_Y: i32 = WORLD_HEIGHT as i32 - 1;

/// Frequency of the cheese fBm, in cycles per block (`F2.1`).
const CHEESE_FREQ: f32 = 1.0 / 48.0;

/// Frequency of the spaghetti fields, in cycles per block (`F2.1`).
const SPAGHETTI_FREQ: f32 = 1.0 / 64.0;

/// Frequency of the entrance field, in cycles per block (`F2.5`).
const ENTRANCE_FREQ: f32 = 1.0 / 192.0;

/// Frequency of the detail field, in cycles per block (`F3.2`).
const DETAIL_FREQ: f32 = 1.0 / 40.0;

/// Noise distance from neutral at which a spaghetti tunnel closes (`F2.1`).
///
/// The carve threshold is `cave_t`, so the surviving tunnel is roughly the inner
/// half of this band. At `1/64` and the default threshold that is a radius of
/// several blocks; `tunnel_radius_at_least_three_blocks` measures it.
const SPAGHETTI_RADIUS: f32 = 0.36;

// Cheese depth window (`F2.3`): `d = top_solid_y - y`.
/// Cheese is absent above this many blocks below the top.
const CHEESE_DEPTH_START: i32 = 12;
/// Cheese reaches full weight at this depth.
const CHEESE_DEPTH_FULL: i32 = 40;
/// Full weight is held to this depth.
const CHEESE_DEPTH_FLAT_END: i32 = 140;
/// Cheese is absent from this depth down.
const CHEESE_DEPTH_END: i32 = 200;

// Cheese floor window (`F2.3`): absolute `y`.
/// Cheese is absent below this `y`.
const CHEESE_FLOOR_START: i32 = 8;
/// Cheese reaches full weight at this `y`.
const CHEESE_FLOOR_FULL: i32 = 24;

/// Lowest block either cave family may carve (`F2.3`: weights are zero at
/// `y = 0..7`).
///
/// The lattice weights are sampled at `4k + 0.5`, so interpolation alone lets a
/// block at `y = 7` borrow a quarter of the cell at `y = 8.5`. This hard gate is
/// what makes the stated block-level floor exact.
const CAVE_FLOOR_Y: i32 = if CHEESE_FLOOR_START < SPAGHETTI_START_Y {
    CHEESE_FLOOR_START
} else {
    SPAGHETTI_START_Y
};

/// Blocks below a near-sea top where cheese is forced to zero (`F2.3`).
const CHEESE_SEAL_BLOCKS: i32 = 24;

// Spaghetti `y` window (`F2.3`).
/// Spaghetti is absent below this `y`.
const SPAGHETTI_START_Y: i32 = 12;
/// Spaghetti reaches full weight at this `y`.
const SPAGHETTI_FULL_Y: i32 = 24;
/// Full weight is held to this `y`.
const SPAGHETTI_FLAT_END_Y: i32 = 296;
/// Spaghetti is absent from this `y` up.
const SPAGHETTI_END_Y: i32 = 308;

// Spaghetti depth window (`F2.3`).
/// Spaghetti is absent closer than this to the top.
const SPAGHETTI_DEPTH_START: i32 = 4;
/// Spaghetti reaches full weight at this depth.
const SPAGHETTI_DEPTH_FULL: i32 = 16;

/// A lowland top (`top_solid_y <= SEA + this`) uses the capped ceiling.
const LOWLAND_TOP_OFFSET: i32 = 8;

/// Lowland spaghetti ceiling, in blocks below sea (`SEA - 8`).
const LOWLAND_CEILING_DEPTH: i32 = 8;

/// Width of the entrance ramp above `entrance_t` (`F2.5`).
const ENTRANCE_RAMP: f32 = 0.05;

/// Threshold reduction at a fully open mouth (`F2.5`, "eases the density threshold").
const MOUTH_EASE: f32 = 0.25;

/// Blocks below sea at which the detail band is fully faded out (`F3.4`).
const DETAIL_COAST_OFFSET: i32 = 2;

/// Blocks over which the detail coast mask fades in.
const DETAIL_COAST_FADE: f32 = 24.0;

/// Divisor of the detail height term (`F3.2`).
const DETAIL_SLOPE_SCALE: f32 = 8.0;

const _: () = assert!(DETAIL_COAST_OFFSET > 0);

/// Entrance curve: x is the raw field in `[0, 1]` (`F2.5`).
///
/// The curve is what makes `entrance_t = 0.82` a rare event on a field whose
/// `unit()` value is centred near 0.5.
const ENTRANCE_KNOTS: &[(f32, f32)] = &[
    (0.0, 0.0),
    (0.40, 0.08),
    (0.55, 0.35),
    (0.65, 0.72),
    (0.72, 0.90),
    (0.80, 0.97),
    (1.0, 1.0),
];

/// Detail curve: x is column roughness, y is the mask (`F3.2`).
const DETAIL_KNOTS: &[(f32, f32)] = &[
    (0.0, 0.0),
    (0.30, 0.15),
    (0.60, 0.55),
    (0.85, 0.90),
    (1.0, 1.0),
];

/// Slope curve: x is the column's integer slope drop, y is the mask (`F3.2`).
const SLOPE_KNOTS: &[(f32, f32)] = &[(0.0, 0.0), (1.0, 0.30), (2.0, 0.65), (4.0, 1.0), (8.0, 1.0)];

/// One block span of lattice cells a column may skip (`F1.3`).
///
/// Both ends are world block `y`, inclusive. [`skippable_band`] returns the span
/// above the cave roof; everything below `y = 1` is skipped as well, because the
/// bedrock block at `y = 0` is never carved.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct SpanSkip {
    /// First block `y` that is never sampled.
    pub y_min: i32,
    /// Last block `y` that is never sampled.
    pub y_max: i32,
}

/// The block band above a column's cave roof that needs no lattice sample (`F1.3`).
///
/// A lattice cell is skipped when its whole four-block neighbourhood sits above
/// `top - CAVE_MIN_ROOF`, which is exactly `y >= y_min`; the `y = 0` cell is *not*
/// skipped because it also holds `y = 1..=3`.
#[must_use]
pub fn skippable_band(top: i32) -> SpanSkip {
    SpanSkip {
        y_min: top.saturating_sub(CAVE_MIN_ROOF).saturating_add(1),
        y_max: WORLD_LAST_Y,
    }
}

/// Whether the lattice cell containing blocks `4c ..= 4c + 3` is skipped (`F1.3`).
///
/// `top` is the column's highest solid block after the detail pass. A cell is
/// skipped when its four-block neighbourhood is entirely above the cave roof or
/// entirely below `CAVE_FLOOR_Y` (the `F2.3` floor, `y = 8`).
#[must_use]
pub fn cell_skipped(cell: i32, top: i32) -> bool {
    let first = cell.saturating_mul(MACRO_CELL);
    first >= skippable_band(top).y_min || first.saturating_add(MACRO_CELL - 1) < CAVE_FLOOR_Y
}

/// The lattice cell of one block coordinate and its exact quarter weight (`B4.5`).
///
/// This is the `y` axis of [`crate::lattice::macro_cell`] and must agree with it
/// bit for bit: `(floor_div(y, 4), rem_euclid(y, 4) / 4)`. A test pins the two
/// together on both sides of the origin.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn lattice_cell(y: i32) -> (i32, f32) {
    let cell = y.div_euclid(MACRO_CELL);
    let weight = y.rem_euclid(MACRO_CELL) as f32 / MACRO_CELL as f32;
    (cell, weight)
}

/// Tunable volume parameters. Every field is a required key in the description.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VolumeParams {
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
    pub fn v1_default(seed: u64) -> Self {
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
    pub fn validate(&self) -> Result<(), TerrainError> {
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
pub struct ColumnInput {
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
pub struct VolumeStats {
    /// Cells the cave pass turned from solid to air.
    pub cave_cells: u32,
    /// Cells the detail pass changed, in either direction.
    pub detail_cells: u32,
    /// Noise taps the whole column spent.
    pub taps: u32,
    /// `1` when this column opened a mouth, else `0`.
    pub mouths: u32,
}

/// The two densities of one 3D cave sample and the taps it cost (`F2.1`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaveSample {
    /// `max(cheese, spaghetti)`, in `[0, 1]`.
    pub density: f32,
    /// Noise taps spent: always [`CAVE_TAP_BUDGET`].
    pub taps: u8,
    /// Whether the final clamp had to fire. The contract is that it never does.
    pub clamped: bool,
}

/// One detail-band contribution and the taps it cost (`F3.2`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetailSample {
    /// Signed density the band adds: `amp * fade * mask * n3`.
    pub delta: f32,
    /// Band weight in `[0, 1]`, zero at both band edges.
    pub fade: f32,
    /// Field mask in `[0, 1]`: roughness, coast, and slope together.
    pub mask: f32,
    /// Noise taps spent.
    pub taps: u8,
}

/// What a solid span is, relative to the cave void that touches it (`F4.1`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum SpanKind {
    /// No cave void touches the span: an ordinary column, or a fully buried layer.
    Surface,
    /// A cave void sits directly above the span's top, which is a cave floor.
    CaveFloor,
    /// A cave void sits directly below the span's bottom, and not above it: the
    /// span's underside is a cave ceiling.
    Ceiling,
}

/// One maximal run of solid blocks (`F4.1`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Span {
    /// Lowest block of the run, inclusive.
    pub y0: i32,
    /// Highest block of the run, inclusive.
    pub y1: i32,
    /// The run's role against the void that touches it.
    pub kind: SpanKind,
}

/// Which `E4.5` rule set a span's top takes (`F4.1`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum SpanRule {
    /// The full `E4.5` column stack: bed, sea floor, beach, snow, slope, biome.
    Stack,
    /// Cave floor open to the sky: stone or gravel, plus biome topsoil, never snow
    /// or beach.
    OpenFloor,
    /// Roofed cave floor: stone or gravel only.
    CaveFloor,
    /// Cave ceiling underside: always stone.
    Stone,
}

/// The span's top rule set (`F4.1`).
///
/// `highest` marks the span at the top of the column: `F4.1` keeps the full `E4.5`
/// stack there whatever void it roofs, so it always answers [`SpanRule::Stack`].
/// Every lower span takes the reduced set: a roofed floor is stone or gravel only,
/// an open floor may also take biome topsoil, and a ceiling is stone. Snow and
/// beach exist only in [`SpanRule::Stack`], which is why a cave floor can never
/// take either.
#[must_use]
pub fn span_rule(kind: SpanKind, highest: bool, open_to_sky: bool) -> SpanRule {
    if highest {
        return SpanRule::Stack;
    }
    match kind {
        SpanKind::Surface => SpanRule::Stack,
        SpanKind::CaveFloor => {
            if open_to_sky {
                SpanRule::OpenFloor
            } else {
                SpanRule::CaveFloor
            }
        }
        SpanKind::Ceiling => SpanRule::Stone,
    }
}

/// The maximal solid runs of a finished column, top first (`F4.1`).
///
/// `air` is the caller's air id. A column with no solid block yields an empty
/// vector; a column with no cave yields exactly one [`SpanKind::Surface`] run whose
/// top is the column's top. Runs are maximal, so any run but the first has a void
/// above it and any run but the last has a void below it.
#[must_use]
pub fn resolve_spans(column: &[BlockId], air: BlockId) -> Vec<Span> {
    let mut spans = Vec::new();
    let len = i32::try_from(column.len()).unwrap_or(i32::MAX);
    let mut y = len - 1;
    while y >= 0 {
        let index = usize::try_from(y).unwrap_or(0);
        if column[index] == air {
            y -= 1;
            continue;
        }
        let y1 = y;
        while y > 0 && column[usize::try_from(y - 1).unwrap_or(0)] != air {
            y -= 1;
        }
        spans.push(Span {
            y0: y,
            y1,
            kind: SpanKind::Surface,
        });
        y -= 1;
    }

    let count = spans.len();
    for (position, span) in spans.iter_mut().enumerate() {
        let above = position > 0;
        let below = position + 1 < count;
        span.kind = match (above, below) {
            (false, false) => SpanKind::Surface,
            (true, _) => SpanKind::CaveFloor,
            (false, true) => SpanKind::Ceiling,
        };
    }
    spans
}

/// Whether everything above `top` is air, i.e. skylight reaches `top` (`F4.1`).
///
/// This is a walk to the top of the column, not a span count: a shaft that reaches
/// the sky is open however many spans surround it, and a pocket roofed by a single
/// block is not.
#[must_use]
pub fn open_to_sky(column: &[BlockId], air: BlockId, top: i32) -> bool {
    let start = usize::try_from(top.saturating_add(1)).unwrap_or(usize::MAX);
    let start = start.min(column.len());
    column[start..].iter().all(|cell| *cell == air)
}

/// The 3D passes for one world: parameters, compiled curves, and keyed streams.
///
/// Construct once with [`Volume::new`] and reuse it for every column. The curves
/// and the stream ids are pure functions of the parameters and the seed, so the
/// struct is `Send` and gives the same answer from any thread.
#[derive(Clone, Debug)]
pub struct Volume {
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
    pub fn new(params: VolumeParams) -> Result<Self, TerrainError> {
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
    pub fn params(&self) -> &VolumeParams {
        &self.params
    }

    /// The entrance field at one column, in `[0, 1]` (`F2.5`).
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn entrance(&self, x: i32, z: i32) -> f32 {
        let raw = fbm_2d(
            self.entrance_seed,
            x as f32,
            z as f32,
            ENTRANCE_FREQ,
            2,
            2.0,
            0.5,
        );
        self.entrance_curve.sample(unit(raw))
    }

    /// Whether the entrance field flags this column (`F2.5`).
    ///
    /// This is the coordinate half of the mouth test: the field crossing
    /// `entrance_t`. Slope, height, and the river/lake/bank exclusions live in
    /// [`Volume::mouth_open`], because they are column facts rather than field
    /// facts. A caller that keeps the flag for the span pass wants
    /// `is_mouth(x, z) && input.slope >= MOUTH_SLOPE_MIN`, or simply the value
    /// [`Volume::mouth_open`] returns.
    #[must_use]
    pub fn is_mouth(&self, x: i32, z: i32) -> bool {
        self.entrance(x, z) >= self.params.entrance_t
    }

    /// The complete `F2.5` mouth gate for one column.
    ///
    /// True when the entrance field is high **and** the column has the required
    /// slope **and** none of the hard exclusions applies. The exclusions are never
    /// overridden by the mask: a flat column, a column at or below `SEA + 1`, a
    /// river bed, a lake bed, and a bank all keep the default roof.
    #[must_use]
    pub fn mouth_open(&self, input: &ColumnInput) -> bool {
        self.is_mouth(input.x, input.z)
            && input.slope >= MOUTH_SLOPE_MIN
            && input.top > self.params.sea + 1
            && !input.is_river
            && !input.is_lake
            && !input.near_bank
    }

    /// The roof thickness in blocks for one column (`F2.5`).
    ///
    /// [`crate::description::CAVE_MIN_ROOF`] unless [`Volume::mouth_open`] holds, in
    /// which case the entrance ramp lowers it towards zero.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn roof(&self, input: &ColumnInput) -> f32 {
        let default = self.params.cave_min_roof as f32;
        if !self.mouth_open(input) {
            return default;
        }
        lerp(default, 0.0, self.entrance_ease(input))
    }

    /// The two cave densities at one 3D position, and their tap cost (`F2.1`).
    ///
    /// `x`, `y`, and `z` are world sample positions, not block indices; a lattice
    /// sample passes `4k + 0.5` on every axis. `top` is the column's
    /// `HeightTile` top; the `F2.3` depth weights are a function of `(top, y)`
    /// and therefore cost no extra tap.
    #[must_use]
    pub fn cave_density(&self, x: f32, y: f32, z: f32, top: i32) -> CaveSample {
        let cheese = self.cheese(x, y, z, top);
        let spaghetti = self.spaghetti(x, y, z, top);
        let density = cheese.max(spaghetti);
        CaveSample {
            density: density.clamp(0.0, 1.0),
            taps: CAVE_TAP_BUDGET,
            clamped: !(0.0..=1.0).contains(&density),
        }
    }

    /// The detail contribution of one block (`F3.2`).
    ///
    /// `x`, `y`, and `z` are block indices; the noise sample sits at the block
    /// centre. The fade uses the block index so both band edges are exactly zero.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn detail_sample(&self, x: i32, y: i32, z: i32, input: &ColumnInput) -> DetailSample {
        let fade = band_fade(input.top, self.params.detail_down, self.params.detail_up, y);
        let coast = ramp(
            (self.params.sea + DETAIL_COAST_OFFSET) as f32,
            (self.params.sea + DETAIL_COAST_OFFSET) as f32 + DETAIL_COAST_FADE,
            input.macro_height,
        );
        let mask = self.detail_curve.sample(input.rough)
            * coast
            * self.slope_curve.sample(input.slope as f32);
        let sample = simplex2_3d(
            self.detail_seed,
            (x as f32 + 0.5) * DETAIL_FREQ,
            (y as f32 + 0.5) * DETAIL_FREQ,
            (z as f32 + 0.5) * DETAIL_FREQ,
        );
        DetailSample {
            delta: self.params.detail_amp * fade * mask * sample,
            fade,
            mask,
            taps: 1,
        }
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
    pub fn apply(
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

    /// `F1.5` step 3: edit solidity inside the detail band.
    fn detail_pass(
        &self,
        column: &mut [BlockId; COLUMN_HEIGHT],
        input: &ColumnInput,
        air: BlockId,
        stone: BlockId,
        stats: &mut VolumeStats,
    ) {
        if !self.detail_allowed(input) {
            return;
        }
        let low = input.top.saturating_sub(self.params.detail_down).max(1);
        let high = input
            .top
            .saturating_add(self.params.detail_up)
            .min(WORLD_LAST_Y - 1);
        if low > high {
            return;
        }
        for y in low..=high {
            let sample = self.detail_sample(input.x, y, input.z, input);
            stats.taps += u32::from(sample.taps);
            #[allow(clippy::cast_precision_loss)]
            let base =
                ((input.macro_height - (y as f32 + 0.5)) / DETAIL_SLOPE_SCALE).clamp(-1.0, 1.0);
            let density = base + sample.delta;
            let want_solid = if base <= 0.0 {
                density >= self.params.float_min
            } else {
                density > 0.0
            };
            let index = usize::try_from(y).unwrap_or(0);
            if want_solid && column[index] == air {
                column[index] = stone;
                stats.detail_cells += 1;
            } else if !want_solid && column[index] != air {
                column[index] = air;
                stats.detail_cells += 1;
            }
        }
    }

    /// `F1.5` step 4: carve cheese and spaghetti, roof-gated and bedrock-safe.
    fn cave_pass(
        &self,
        column: &mut [BlockId; COLUMN_HEIGHT],
        input: &ColumnInput,
        air: BlockId,
        stats: &mut VolumeStats,
    ) {
        let Some(solid_top) = highest_solid(column, air) else {
            return;
        };
        let mouth = self.mouth_open(input);
        if mouth {
            stats.mouths += 1;
        }
        #[allow(clippy::cast_precision_loss)]
        let roof = self.roof(input);
        let threshold = self.threshold(input);
        let mut lattice = LatticeColumn::new(self, input.x, input.z, input.top);
        for y in CAVE_FLOOR_Y.max(1)..=solid_top {
            #[allow(clippy::cast_precision_loss)]
            let clearance = (solid_top - y) as f32;
            if clearance < roof {
                continue;
            }
            let (cell, weight) = lattice_cell(y);
            let near = lattice.density(cell);
            let density = if weight > 0.0 {
                let far = lattice.next_density();
                near + (far - near) * weight
            } else {
                near
            };
            let index = usize::try_from(y).unwrap_or(0);
            if density > threshold && column[index] != air {
                column[index] = air;
                stats.cave_cells += 1;
            }
        }
        stats.taps = stats.taps.saturating_add(lattice.taps);
    }

    /// `F3.4`: detail has no business over shallow water, a bed, or a bank.
    fn detail_allowed(&self, input: &ColumnInput) -> bool {
        input.top > self.params.sea + DETAIL_COAST_OFFSET
            && !input.is_river
            && !input.is_lake
            && !input.near_bank
    }

    /// How far up the entrance ramp this column sits, in `[0, 1]` (`F2.5`).
    fn entrance_ease(&self, input: &ColumnInput) -> f32 {
        let field = self.entrance(input.x, input.z);
        smoothstep(
            self.params.entrance_t,
            self.params.entrance_t + ENTRANCE_RAMP,
            field,
        )
    }

    /// The density threshold for one column (`F2.5`).
    fn threshold(&self, input: &ColumnInput) -> f32 {
        if !self.mouth_open(input) {
            return self.params.cave_t;
        }
        (self.params.cave_t - MOUTH_EASE * self.entrance_ease(input)).max(0.0)
    }

    /// Cheese: two-octave fBm at `1/48`, scaled by the `F2.3` windows.
    #[allow(clippy::cast_precision_loss)]
    fn cheese(&self, x: f32, y: f32, z: f32, top: i32) -> f32 {
        let frequency = CHEESE_FREQ;
        let first = simplex2_3d(
            self.cheese_seeds[0],
            x * frequency,
            y * frequency,
            z * frequency,
        );
        let second = simplex2_3d(
            self.cheese_seeds[1],
            x * frequency * 2.0,
            y * frequency * 2.0,
            z * frequency * 2.0,
        );
        let fbm = (first + 0.5 * second) * (1.0 / 1.5);
        unit(fbm) * self.cheese_weight(top, y)
    }

    /// Spaghetti: two single-octave fields whose zero set is a tunnel (`F2.1`).
    #[allow(clippy::many_single_char_names)]
    fn spaghetti(&self, x: f32, y: f32, z: f32, top: i32) -> f32 {
        let frequency = SPAGHETTI_FREQ;
        let a = unit(simplex2_3d(
            self.spaghetti_seeds[0],
            x * frequency,
            y * frequency,
            z * frequency,
        ));
        let b = unit(simplex2_3d(
            self.spaghetti_seeds[1],
            x * frequency,
            y * frequency,
            z * frequency,
        ));
        let distance = ((a - 0.5).abs() * 2.0).max((b - 0.5).abs() * 2.0);
        let tunnel = 1.0 - smooth01(distance / SPAGHETTI_RADIUS);
        tunnel * self.spaghetti_weight(top, y)
    }

    /// The `F2.3` cheese weight at one `(top, y)`.
    ///
    /// Product of the depth trapezoid, the absolute-`y` floor, and the
    /// near-sea seal. All three are monotone factors; the seal is a hard zero
    /// because a beach or sea floor within 24 blocks of its top must stay solid.
    #[allow(clippy::cast_precision_loss)]
    fn cheese_weight(&self, top: i32, y: f32) -> f32 {
        if top <= self.params.sea.saturating_add(1)
            && y > top.saturating_sub(CHEESE_SEAL_BLOCKS) as f32
        {
            return 0.0;
        }
        let depth = top as f32 - y;
        cheese_depth_weight(depth) * cheese_floor_weight(y)
    }

    /// The `F2.3` spaghetti weight at one `(top, y)`.
    ///
    /// Product of the `y` trapezoid and the depth ramp. A lowland top
    /// (`top <= SEA + 8`) caps the ceiling at `SEA - 8`, so a coastal plain has
    /// no shallow tunnels; a highland column keeps the full 296..308 taper.
    #[allow(clippy::cast_precision_loss)]
    fn spaghetti_weight(&self, top: i32, y: f32) -> f32 {
        let ceiling = self.params.sea - LOWLAND_CEILING_DEPTH;
        if top <= self.params.sea + LOWLAND_TOP_OFFSET && y > ceiling as f32 {
            return 0.0;
        }
        let depth = top as f32 - y;
        spaghetti_y_weight(y) * spaghetti_depth_weight(depth)
    }
}

/// Cheese depth trapezoid: `0` below 12, `1` by 40, flat to 140, `0` by 200.
#[allow(clippy::cast_precision_loss)]
fn cheese_depth_weight(depth: f32) -> f32 {
    ramp(CHEESE_DEPTH_START as f32, CHEESE_DEPTH_FULL as f32, depth)
        * (1.0 - ramp(CHEESE_DEPTH_FLAT_END as f32, CHEESE_DEPTH_END as f32, depth))
}

/// Cheese floor ramp: `0` below `y = 8`, `1` by `y = 24`.
#[allow(clippy::cast_precision_loss)]
fn cheese_floor_weight(y: f32) -> f32 {
    ramp(CHEESE_FLOOR_START as f32, CHEESE_FLOOR_FULL as f32, y)
}

/// Spaghetti `y` trapezoid: `0` below 12, `1` from 24, flat to 296, `0` by 308.
#[allow(clippy::cast_precision_loss)]
fn spaghetti_y_weight(y: f32) -> f32 {
    ramp(SPAGHETTI_START_Y as f32, SPAGHETTI_FULL_Y as f32, y)
        * (1.0 - ramp(SPAGHETTI_FLAT_END_Y as f32, SPAGHETTI_END_Y as f32, y))
}

/// Spaghetti depth ramp: `0` at `d < 4`, `1` by `d = 16`.
#[allow(clippy::cast_precision_loss)]
fn spaghetti_depth_weight(depth: f32) -> f32 {
    ramp(
        SPAGHETTI_DEPTH_START as f32,
        SPAGHETTI_DEPTH_FULL as f32,
        depth,
    )
}

/// One column of the `y` lattice with a two-cell cache (`F1.3`).
///
/// Consecutive blocks walk the cells in order, so a cell is evaluated exactly once
/// and never at all when the roof test already excluded it.
struct LatticeColumn<'a> {
    /// The volume that owns the noise streams.
    volume: &'a Volume,
    /// World block `x` of the column.
    x: i32,
    /// World block `z` of the column.
    z: i32,
    /// The column's pre-detail `HeightTile` top (`F2.3` weights).
    top: i32,
    /// Cell whose value is cached in `value`.
    cell: i32,
    /// Cached value of `cell`.
    value: f32,
    /// Cell whose value is cached in `next_value`.
    next_cell: i32,
    /// Cached value of `next_cell`.
    next_value: f32,
    /// Taps spent by this column so far.
    taps: u32,
}

impl<'a> LatticeColumn<'a> {
    /// A cache with no cell loaded.
    fn new(volume: &'a Volume, x: i32, z: i32, top: i32) -> Self {
        Self {
            volume,
            x,
            z,
            top,
            cell: i32::MIN,
            value: 0.0,
            next_cell: i32::MIN,
            next_value: 0.0,
            taps: 0,
        }
    }

    /// The density at `cell`, promoting a cached neighbour when it matches.
    fn density(&mut self, cell: i32) -> f32 {
        if cell == self.cell {
            return self.value;
        }
        if cell == self.next_cell {
            self.cell = self.next_cell;
            self.value = self.next_value;
            self.next_cell = i32::MIN;
            return self.value;
        }
        self.cell = cell;
        self.value = self.evaluate(cell);
        self.next_cell = i32::MIN;
        self.value
    }

    /// The density at `cell + 1`, evaluated at most once per cell.
    fn next_density(&mut self) -> f32 {
        let wanted = self.cell.saturating_add(1);
        if self.next_cell != wanted {
            self.next_cell = wanted;
            self.next_value = self.evaluate(wanted);
        }
        self.next_value
    }

    /// One lattice sample: four taps at the cell centre (`F2.1`).
    #[allow(clippy::cast_precision_loss)]
    fn evaluate(&mut self, cell: i32) -> f32 {
        let y = (cell.saturating_mul(MACRO_CELL)) as f32 + 0.5;
        let sample =
            self.volume
                .cave_density(self.x as f32 + 0.5, y, self.z as f32 + 0.5, self.top);
        self.taps = self.taps.saturating_add(u32::from(sample.taps));
        sample.density
    }
}

/// The highest solid block of a column, if it has one.
fn highest_solid(column: &[BlockId; COLUMN_HEIGHT], air: BlockId) -> Option<i32> {
    for y in (1..COLUMN_HEIGHT).rev() {
        if column[y] != air {
            return i32::try_from(y).ok();
        }
    }
    None
}

/// Fixed-order linear interpolation. No `mul_add`.
fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

/// Hermite smoothstep on `[0, 1]`, clamped outside.
fn smooth01(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Hermite smoothstep between two edges, clamped outside.
fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let span = edge1 - edge0;
    if span <= 0.0 {
        return if value < edge0 { 0.0 } else { 1.0 };
    }
    smooth01((value - edge0) / span)
}

/// A clamped linear ramp from `low` to `high`.
fn ramp(low: f32, high: f32, value: f32) -> f32 {
    let span = high - low;
    if span <= 0.0 {
        return if value < low { 0.0 } else { 1.0 };
    }
    ((value - low) / span).clamp(0.0, 1.0)
}

/// The detail band weight of one block: zero at both edges, one at the top (`F3.2`).
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn band_fade(top: i32, down: i32, up: i32, y: i32) -> f32 {
    let low = top.saturating_sub(down);
    let high = top.saturating_add(up);
    if y < low || y > high {
        return 0.0;
    }
    let below = if down > 0 {
        (y - low) as f32 / down as f32
    } else {
        1.0
    };
    let above = if up > 0 {
        (high - y) as f32 / up as f32
    } else {
        1.0
    };
    below.min(above).clamp(0.0, 1.0)
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

#[cfg(test)]
mod tests {
    #![allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss
    )]

    use super::*;
    use crate::hash::{hash01, hash3};
    use crate::lattice::macro_cell;

    /// Air, as the caller supplies it.
    const AIR: BlockId = 0;
    /// Stone, as the caller supplies it.
    const STONE: BlockId = 7;

    fn volume(seed: u64) -> Volume {
        Volume::new(VolumeParams::v1_default(seed)).expect("the v1 defaults are valid")
    }

    fn land(x: i32, z: i32, top: i32) -> ColumnInput {
        ColumnInput {
            x,
            z,
            top,
            rough: 1.0,
            slope: 8,
            is_river: false,
            is_lake: false,
            near_bank: false,
            #[allow(clippy::cast_precision_loss)]
            macro_height: top as f32 + 0.4,
        }
    }

    fn solid_column(top: i32) -> [BlockId; COLUMN_HEIGHT] {
        let mut column = [AIR; COLUMN_HEIGHT];
        for y in 0..=top {
            column[usize::try_from(y).unwrap()] = STONE;
        }
        column
    }

    /// One deterministic coordinate triple per index.
    fn sample_coords(seed: u64, index: i32) -> (f32, f32, f32) {
        let tag = domain("volume-test-samples");
        let x = hash01(hash3(seed, tag, index, 0, 1)) * 4096.0 - 2048.0;
        let z = hash01(hash3(seed, tag, index, 2, 3)) * 4096.0 - 2048.0;
        let y = 1.0 + hash01(hash3(seed, tag, index, 4, 5)) * 350.0;
        (x, y, z)
    }

    #[test]
    fn density_stays_in_range() {
        let volume = volume(0x5EED);
        let mut clamps = 0_u32;
        let mut carved = 0_u32;
        for index in 0..100_000 {
            let (x, y, z) = sample_coords(7, index);
            let sample = volume.cave_density(x, y, z, crate::description::MAX_TERRAIN_Y);
            assert!(
                (0.0..=1.0).contains(&sample.density),
                "{} at {x} {y} {z}",
                sample.density
            );
            clamps += u32::from(sample.clamped);
            carved += u32::from(sample.density > volume.params().cave_t);
            assert_eq!(
                sample.taps, CAVE_TAP_BUDGET,
                "a 3D sample must spend exactly the budget"
            );
        }
        assert_eq!(clamps, 0, "unit()'s clamp is a backstop and must not fire");
        assert!(carved > 0, "the fixture must carve somewhere");
        assert!(
            carved < 100_000 / 2,
            "cheese must not fill half the underground: {carved}"
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn cave_window_curves_are_monotone() {
        // Cheese depth: rises to 40, flat to 140, falls to zero by 200.
        let mut previous = cheese_depth_weight(0.0);
        assert_eq!(previous, 0.0, "no cheese above d = 12");
        for depth in 1..=CHEESE_DEPTH_FULL {
            let weight = cheese_depth_weight(depth as f32);
            assert!(weight >= previous, "cheese depth fell at {depth}");
            previous = weight;
        }
        assert_eq!(cheese_depth_weight(CHEESE_DEPTH_FULL as f32), 1.0);
        for depth in CHEESE_DEPTH_FULL..=CHEESE_DEPTH_FLAT_END {
            assert_eq!(
                cheese_depth_weight(depth as f32),
                1.0,
                "cheese depth is not flat at {depth}"
            );
        }
        previous = 1.0;
        for depth in CHEESE_DEPTH_FLAT_END..=CHEESE_DEPTH_END {
            let weight = cheese_depth_weight(depth as f32);
            assert!(weight <= previous, "cheese tail rose at {depth}");
            previous = weight;
        }
        assert_eq!(cheese_depth_weight(CHEESE_DEPTH_END as f32), 0.0);

        // Cheese floor: zero below 8, full by 24.
        let mut previous = cheese_floor_weight(0.0);
        assert_eq!(previous, 0.0);
        for y in 1..=CHEESE_FLOOR_FULL {
            let weight = cheese_floor_weight(y as f32);
            assert!(weight >= previous, "cheese floor fell at {y}");
            previous = weight;
        }
        assert_eq!(cheese_floor_weight(CHEESE_FLOOR_FULL as f32), 1.0);

        // Spaghetti `y`: rises to 24, flat to 296, falls to zero by 308.
        let mut previous = spaghetti_y_weight(0.0);
        assert_eq!(previous, 0.0);
        for y in 1..=SPAGHETTI_FULL_Y {
            let weight = spaghetti_y_weight(y as f32);
            assert!(weight >= previous, "spaghetti y fell at {y}");
            previous = weight;
        }
        assert_eq!(spaghetti_y_weight(SPAGHETTI_FULL_Y as f32), 1.0);
        for y in SPAGHETTI_FULL_Y..=SPAGHETTI_FLAT_END_Y {
            assert_eq!(spaghetti_y_weight(y as f32), 1.0, "not flat at {y}");
        }
        previous = 1.0;
        for y in SPAGHETTI_FLAT_END_Y..=SPAGHETTI_END_Y {
            let weight = spaghetti_y_weight(y as f32);
            assert!(weight <= previous, "spaghetti tail rose at {y}");
            previous = weight;
        }
        assert_eq!(spaghetti_y_weight(SPAGHETTI_END_Y as f32), 0.0);

        // Spaghetti depth: zero at d < 4, full by 16.
        let mut previous = spaghetti_depth_weight(0.0);
        assert_eq!(previous, 0.0);
        for depth in 1..=SPAGHETTI_DEPTH_FULL {
            let weight = spaghetti_depth_weight(depth as f32);
            assert!(weight >= previous, "spaghetti depth fell at {depth}");
            previous = weight;
        }
        assert_eq!(spaghetti_depth_weight(SPAGHETTI_DEPTH_FULL as f32), 1.0);
        assert_eq!(spaghetti_depth_weight(40.0), 1.0);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn no_cheese_below_the_floor_or_near_the_top() {
        let volume = volume(0x5EED);
        let top = 60;
        for y in 0..CHEESE_FLOOR_START {
            assert_eq!(
                volume.cheese_weight(top, y as f32),
                0.0,
                "cheese at y = {y}"
            );
        }
        assert!(volume.cheese_weight(top, CHEESE_FLOOR_FULL as f32) > 0.0);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_near_sea_column_stays_sealed_under_its_surface() {
        let volume = volume(0x5EED);
        let top = SEA + 1;

        // Cheese is zero for the whole 24 blocks under a beach or sea floor
        // (the cell exactly 24 blocks down is at the boundary, not inside it).
        for y in (top - CHEESE_SEAL_BLOCKS + 1)..=top {
            assert_eq!(
                volume.cheese_weight(top, y as f32),
                0.0,
                "cheese breached the seal at y = {y}"
            );
        }
        assert!(
            volume.cheese_weight(top, (top - CHEESE_SEAL_BLOCKS - 1) as f32) > 0.0
                || top - CHEESE_SEAL_BLOCKS - 1 < CHEESE_FLOOR_START,
            "the seal must be a local rule, not a global zero"
        );

        // End to end with an aggressive threshold: no carve above the lowland
        // spaghetti ceiling, so the sea floor stays sealed.
        let aggressive = Volume::new(VolumeParams {
            cave_t: 0.0,
            detail_amp: 0.0,
            ..VolumeParams::v1_default(0x5EED)
        })
        .expect("valid");
        let input = ColumnInput {
            slope: 0,
            rough: 0.0,
            ..land(3, 4, top)
        };
        let mut column = solid_column(top);
        aggressive.apply(&mut column, &input, AIR, STONE);
        // The combined density is what carves, and spaghetti is still allowed
        // below the lowland ceiling; above it the sea floor is sealed.
        for y in (SEA - LOWLAND_CEILING_DEPTH + 1)..=top {
            assert_ne!(
                column[usize::try_from(y).unwrap()],
                AIR,
                "lowland ceiling allowed a tunnel at y = {y}"
            );
        }
    }

    #[test]
    fn nothing_carves_within_four_blocks_of_the_surface_except_mouths() {
        let volume = Volume::new(VolumeParams {
            cave_t: 0.0,
            detail_amp: 0.0,
            ..VolumeParams::v1_default(0x5EED)
        })
        .expect("valid");

        let mut quiet = None;
        'scan: for z in 0..96 {
            for x in 0..96 {
                // `macro_height` sits just above the integer top so the detail
                // pass, which is disabled by amplitude but still evaluates its
                // base term, keeps the top block.
                let input = ColumnInput {
                    slope: 0,
                    rough: 0.0,
                    macro_height: 200.6,
                    ..land(x, z, 200)
                };
                if !volume.mouth_open(&input) {
                    quiet = Some((x, z, input));
                    break 'scan;
                }
            }
        }
        let (x, z, input) = quiet.expect("the fixture finds a non-mouth column");

        let mut column = solid_column(input.top);
        let stats = volume.apply(&mut column, &input, AIR, STONE);
        assert_eq!(stats.mouths, 0, "a flat column is not a mouth");
        assert!(stats.cave_cells > 0, "the carve must reach somewhere");
        for offset in 0..CAVE_MIN_ROOF {
            let y = input.top - offset;
            assert_ne!(
                column[usize::try_from(y).unwrap()],
                AIR,
                "cave carved {offset} blocks under the surface at {x},{z}"
            );
        }
    }

    #[test]
    fn tap_budget_is_enforced() {
        let volume = volume(11);
        let input = land(3, 4, 180);
        let mut cave_max = 0_u8;
        let mut detail_max = 0_u8;
        for index in 0..10_000 {
            let (x, y, z) = sample_coords(13, index);
            let cave = volume.cave_density(x, y, z, input.top);
            cave_max = cave_max.max(cave.taps);
            let block_x = (index % 64) - 32;
            let block_z = (index / 64) - 32;
            let detail = volume.detail_sample(block_x, 180, block_z, &input);
            detail_max = detail_max.max(detail.taps);
        }
        assert_eq!(cave_max, CAVE_TAP_BUDGET);
        assert!(cave_max <= CAVE_TAP_BUDGET);
        assert_eq!(detail_max, 1);
        assert!(detail_max <= DETAIL_TAP_BUDGET);
        assert!(detail_max > 0);
    }

    #[test]
    fn lattice_skips_above_the_roof() {
        let params = VolumeParams {
            detail_amp: 0.0,
            ..VolumeParams::v1_default(5)
        };
        let volume = Volume::new(params).expect("valid");
        let input = ColumnInput {
            slope: 0,
            rough: 0.0,
            ..land(0, 0, 100)
        };
        let mut column = solid_column(100);
        let stats = volume.apply(&mut column, &input, AIR, STONE);

        assert_eq!(stats.mouths, 0, "a flat column cannot open a mouth");
        assert_eq!(stats.detail_cells, 0, "detail is off in this fixture");
        // Cells 2..=24 are the only ones the floor and roof allow: 23 cells,
        // four taps each. Cells 0 and 1 are below the `F2.3` floor.
        assert_eq!(stats.taps, 23 * u32::from(CAVE_TAP_BUDGET));

        let band = skippable_band(100);
        assert_eq!(band.y_min, 100 - CAVE_MIN_ROOF + 1);
        assert_eq!(band.y_max, WORLD_LAST_Y);
        assert!(cell_skipped(25, 100), "cell 25 starts above the roof");
        assert!(
            !cell_skipped(24, 100),
            "cell 24 still touches the roof band"
        );
        assert!(!cell_skipped(2, 100), "the y = 8 cell is sampled");
        assert!(
            cell_skipped(1, 100),
            "nothing below the F2.3 floor is sampled"
        );
        assert!(cell_skipped(0, 100), "the y = 0 cell is below the floor");
        assert!(cell_skipped(-1, 100), "nothing below y = 1 is sampled");
        for cell in 25..LATTICE_CELLS as i32 {
            assert!(cell_skipped(cell, 100), "cell {cell} is above the roof");
        }
        for cell in -8..0 {
            assert!(cell_skipped(cell, 100), "cell {cell} is below the world");
        }
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn band_fade_is_zero_at_both_edges() {
        let volume = volume(0);
        let input = land(17, 23, 200);
        let low = 200 - DETAIL_DOWN;
        let high = 200 + DETAIL_UP;

        let below = volume.detail_sample(input.x, low, input.z, &input);
        let above = volume.detail_sample(input.x, high, input.z, &input);
        assert_eq!(below.fade, 0.0);
        assert_eq!(above.fade, 0.0);
        assert_eq!(below.delta, 0.0, "the edge contributes exactly nothing");
        assert_eq!(above.delta, 0.0, "the edge contributes exactly nothing");
        assert_eq!(
            volume
                .detail_sample(input.x, low - 1, input.z, &input)
                .delta,
            0.0
        );
        assert_eq!(
            volume
                .detail_sample(input.x, high + 1, input.z, &input)
                .delta,
            0.0
        );

        // Somewhere on the band the noise term is positive; the centre is the
        // natural place, and the fixture picks a column where n3 is positive.
        let mut found = false;
        'scan: for z in -16..16 {
            for x in -16..16 {
                let centre = volume.detail_sample(x, 200, z, &input);
                if centre.fade == 1.0 && centre.delta > 0.0 {
                    found = true;
                    break 'scan;
                }
            }
        }
        assert!(found, "the detail centre must be capable of raising rock");
    }

    #[test]
    fn detail_cannot_make_a_floater_smaller_than_float_min() {
        let params = VolumeParams {
            cave_t: 1.0,
            detail_amp: 1.0,
            ..VolumeParams::v1_default(21)
        };
        let volume = Volume::new(params).expect("valid");
        let input = land(0, 0, 200);
        let target = 201;

        let mut small = None;
        let mut large = None;
        'scan: for z in 0..64 {
            for x in 0..64 {
                let sample = volume.detail_sample(x, target, z, &input);
                if small.is_none()
                    && sample.delta > 0.02
                    && sample.delta < volume.params().float_min
                {
                    small = Some((x, z));
                }
                if large.is_none() && sample.delta > 0.4 {
                    large = Some((x, z));
                }
                if small.is_some() && large.is_some() {
                    break 'scan;
                }
            }
        }
        let small = small.expect("the fixture finds a sub-threshold bump");
        let large = large.expect("the fixture finds a super-threshold bump");

        let mut quiet = solid_column(200);
        let quiet_input = ColumnInput {
            x: small.0,
            z: small.1,
            ..input
        };
        volume.apply(&mut quiet, &quiet_input, AIR, STONE);
        assert_eq!(
            quiet[usize::try_from(target).unwrap()],
            AIR,
            "a bump below FLOAT_MIN must not spawn a block"
        );

        let mut loud = solid_column(200);
        let loud_input = ColumnInput {
            x: large.0,
            z: large.1,
            ..input
        };
        volume.apply(&mut loud, &loud_input, AIR, STONE);
        assert_eq!(
            loud[usize::try_from(target).unwrap()],
            STONE,
            "a bump above FLOAT_MIN spawns a floater"
        );

        // And the invariant behind the two cases, end to end: every block the detail
        // pass solidified above the surface cleared the float gate.
        let mut checked = 0;
        for index in 0..256 {
            let (x, _, z) = sample_coords(23, index);
            #[allow(clippy::cast_possible_truncation)]
            let (block_x, block_z) = (x as i32, z as i32);
            let column_input = ColumnInput {
                x: block_x,
                z: block_z,
                ..input
            };
            let mut filled = solid_column(200);
            volume.apply(&mut filled, &column_input, AIR, STONE);
            for y in target..=200 + DETAIL_UP {
                let index = usize::try_from(y).unwrap();
                if filled[index] == AIR {
                    continue;
                }
                let sample = volume.detail_sample(block_x, y, block_z, &column_input);
                #[allow(clippy::cast_precision_loss)]
                let base = ((column_input.macro_height - (y as f32 + 0.5)) / DETAIL_SLOPE_SCALE)
                    .clamp(-1.0, 1.0);
                assert!(
                    base + sample.delta >= volume.params().float_min,
                    "a floater at {block_x} {y} {block_z} is below FLOAT_MIN"
                );
                checked += 1;
            }
        }
        assert!(checked > 0, "the sweep must see at least one floater");
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn entrance_lowers_the_roof_but_not_the_exclusions() {
        let volume = volume(0xABCD);
        let deep_enough = volume.params().entrance_t + ENTRANCE_RAMP;

        let mut mouth = None;
        'scan: for z in 0..96 {
            for x in 0..96 {
                if volume.entrance(x, z) >= deep_enough {
                    mouth = Some((x, z));
                    break 'scan;
                }
            }
        }
        let (x, z) = mouth.expect("the v1 entrance field has mouths in a 96-block window");

        let open = land(x, z, 200);
        assert!(volume.is_mouth(x, z), "the field must flag this column");
        assert!(volume.mouth_open(&open));
        assert!(
            volume.roof(&open) < volume.params().cave_min_roof as f32,
            "the entrance must lower the roof"
        );
        assert!(volume.roof(&open) < 0.05, "a full mouth opens to the sky");

        // Hard exclusions: the mask never overrides any of them.
        let excluded = [
            ColumnInput {
                top: volume.params().sea + 1,
                ..open
            },
            ColumnInput {
                is_river: true,
                ..open
            },
            ColumnInput {
                is_lake: true,
                ..open
            },
            ColumnInput {
                near_bank: true,
                ..open
            },
            ColumnInput {
                slope: MOUTH_SLOPE_MIN - 1,
                ..open
            },
        ];
        for column in excluded {
            assert!(!volume.mouth_open(&column), "{column:?}");
            assert_eq!(volume.roof(&column), volume.params().cave_min_roof as f32);
        }

        // A mouth column counts once, and an excluded one never counts.
        let mut opened = solid_column(200);
        let stats = volume.apply(&mut opened, &open, AIR, STONE);
        assert_eq!(stats.mouths, 1);
        assert!(stats.cave_cells > 0, "an open mouth must carve something");

        let river = ColumnInput {
            is_river: true,
            ..open
        };
        let mut river_column = solid_column(200);
        let river_stats = volume.apply(&mut river_column, &river, AIR, STONE);
        assert_eq!(river_stats.mouths, 0);

        // The `F2.3` floor keeps everything below `y = 8` solid even at the
        // most aggressive threshold; the carve starts above it.
        let aggressive = Volume::new(VolumeParams {
            cave_t: 0.0,
            ..VolumeParams::v1_default(0xABCD)
        })
        .expect("valid");
        let mut floor_column = solid_column(200);
        aggressive.apply(&mut floor_column, &open, AIR, STONE);
        assert_ne!(floor_column[0], AIR, "bedrock is never carved");
        for y in 1..CHEESE_FLOOR_START {
            assert_ne!(
                floor_column[usize::try_from(y).unwrap()],
                AIR,
                "the cheese floor was breached at y = {y}"
            );
        }
        assert!(
            floor_column
                .iter()
                .skip(usize::try_from(CHEESE_FLOOR_START).unwrap())
                .any(|id| *id == AIR),
            "the carve must start above the cheese floor"
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn mouth_requires_slope() {
        let volume = volume(0xABCD);
        let deep_enough = volume.params().entrance_t + ENTRANCE_RAMP;
        let mut mouth = None;
        'scan: for z in 0..96 {
            for x in 0..96 {
                if volume.entrance(x, z) >= deep_enough {
                    mouth = Some((x, z));
                    break 'scan;
                }
            }
        }
        let (x, z) = mouth.expect("the fixture finds a mouth candidate");

        for slope in 0..MOUTH_SLOPE_MIN {
            let flat = ColumnInput {
                slope,
                ..land(x, z, 200)
            };
            assert!(volume.is_mouth(x, z), "the field is still high");
            assert!(!volume.mouth_open(&flat), "slope {slope} is not enough");
            assert_eq!(volume.roof(&flat), volume.params().cave_min_roof as f32);
            let mut column = solid_column(200);
            assert_eq!(volume.apply(&mut column, &flat, AIR, STONE).mouths, 0);
        }

        let steep = ColumnInput {
            slope: MOUTH_SLOPE_MIN,
            ..land(x, z, 200)
        };
        assert!(volume.mouth_open(&steep));
        assert!(volume.roof(&steep) < volume.params().cave_min_roof as f32);
    }

    #[test]
    fn one_span_matches_the_surface_stack() {
        let column = solid_column(100);
        let spans = resolve_spans(&column, AIR);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].y0, 0);
        assert_eq!(spans[0].y1, 100);
        assert_eq!(spans[0].kind, SpanKind::Surface);
        assert_eq!(span_rule(spans[0].kind, true, true), SpanRule::Stack);
    }

    #[test]
    fn spans_split_at_air() {
        let mut column = solid_column(100);
        for y in 60..=69 {
            column[usize::try_from(y).unwrap()] = AIR;
        }
        let spans = resolve_spans(&column, AIR);
        assert_eq!(spans.len(), 2);
        assert_eq!(
            spans[0],
            Span {
                y0: 70,
                y1: 100,
                kind: SpanKind::Ceiling
            }
        );
        assert_eq!(
            spans[1],
            Span {
                y0: 0,
                y1: 59,
                kind: SpanKind::CaveFloor
            }
        );
        // The ceiling keeps the full stack because it is the highest span; the cave
        // floor below it does not.
        assert_eq!(span_rule(spans[0].kind, true, false), SpanRule::Stack);
        assert_eq!(span_rule(spans[1].kind, false, false), SpanRule::CaveFloor);
    }

    #[test]
    fn cave_floor_never_gets_snow_or_beach() {
        let mut column = solid_column(100);
        for y in 80..=89 {
            column[usize::try_from(y).unwrap()] = AIR;
        }
        for y in 40..=49 {
            column[usize::try_from(y).unwrap()] = AIR;
        }
        let spans = resolve_spans(&column, AIR);
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].kind, SpanKind::Ceiling);
        assert_eq!(spans[1].kind, SpanKind::CaveFloor);
        assert_eq!(spans[2].kind, SpanKind::CaveFloor);

        for span in spans.iter().skip(1) {
            let rule = span_rule(span.kind, false, false);
            assert_ne!(rule, SpanRule::Stack, "a lower top never gets the stack");
            assert!(matches!(rule, SpanRule::CaveFloor | SpanRule::Stone));
            assert!(
                span.y1 < spans[0].y1,
                "a lower span must sit below the top span"
            );
        }
        // Only the full stack can produce snow (slot 4) or beach (slot 3).
        assert_eq!(span_rule(SpanKind::Surface, true, false), SpanRule::Stack);
        // An open lower top may take topsoil, but still never snow or beach.
        assert_eq!(
            span_rule(SpanKind::CaveFloor, false, true),
            SpanRule::OpenFloor
        );
        // A lower ceiling is always stone.
        assert_eq!(span_rule(SpanKind::Ceiling, false, true), SpanRule::Stone);
    }

    #[test]
    fn open_to_sky_tracks_the_visible_column() {
        let open = solid_column(100);
        assert!(open_to_sky(&open, AIR, 100));

        // A roofed pocket: the solid above the void blocks the sky.
        let mut pocket = solid_column(100);
        for y in 60..=69 {
            pocket[usize::try_from(y).unwrap()] = AIR;
        }
        assert!(!open_to_sky(&pocket, AIR, 59));
        assert!(open_to_sky(&pocket, AIR, 100));

        // A shaft through the surface: the cell under it is open even though the
        // column above it is mostly solid.
        let mut shaft = solid_column(100);
        for y in 51..=100 {
            shaft[usize::try_from(y).unwrap()] = AIR;
        }
        assert!(open_to_sky(&shaft, AIR, 50));
        assert!(!open_to_sky(&shaft, AIR, 0));

        // A pocket under a one-block roof is not open.
        let mut lid = solid_column(100);
        for y in 61..=99 {
            lid[usize::try_from(y).unwrap()] = AIR;
        }
        assert!(!open_to_sky(&lid, AIR, 60));
    }

    #[test]
    fn caves_never_breach_bedrock() {
        let aggressive = Volume::new(VolumeParams {
            cave_t: 0.2,
            ..VolumeParams::v1_default(101)
        })
        .expect("valid");
        let tag = domain("volume-bedrock-sweep");
        for index in 0..1_000_i32 {
            let x = hash01(hash3(3, tag, index, 0, 9)) * 8192.0 - 4096.0;
            let z = hash01(hash3(3, tag, index, 1, 8)) * 8192.0 - 4096.0;
            #[allow(clippy::cast_possible_truncation)]
            let (block_x, block_z) = (x as i32, z as i32);
            #[allow(clippy::cast_precision_loss)]
            let top = 60 + (hash01(hash3(3, tag, index, 2, 7)) * 260.0) as i32;
            let input = ColumnInput {
                slope: 4,
                rough: 0.8,
                ..land(block_x, block_z, top)
            };
            let mut column = solid_column(top);
            aggressive.apply(&mut column, &input, AIR, STONE);
            assert_ne!(column[0], AIR, "column {index} breached bedrock");
            assert_eq!(column[0], STONE);
        }
    }

    #[test]
    fn lattice_cell_agrees_with_macro_cell() {
        for y in [
            i32::MIN + 1,
            -1025,
            -5,
            -4,
            -1,
            0,
            1,
            3,
            4,
            383,
            4096,
            i32::MAX - 1,
        ] {
            assert_eq!(lattice_cell(y), macro_cell(y), "y = {y}");
        }
        assert_eq!(lattice_cell(-1), (-1, 0.75));
        assert_eq!(lattice_cell(0), (0, 0.0));
        assert_eq!(lattice_cell(3), (0, 0.75));
        assert_eq!(lattice_cell(4), (1, 0.0));
    }

    #[test]
    fn params_validation_rejects_bad_values() {
        assert!(VolumeParams::v1_default(1).validate().is_ok());

        let cases = [
            VolumeParams {
                cave_t: 1.5,
                ..VolumeParams::v1_default(1)
            },
            VolumeParams {
                entrance_t: f32::NAN,
                ..VolumeParams::v1_default(1)
            },
            VolumeParams {
                cave_min_roof: -1,
                ..VolumeParams::v1_default(1)
            },
            VolumeParams {
                detail_down: -1,
                ..VolumeParams::v1_default(1)
            },
            VolumeParams {
                detail_up: -1,
                ..VolumeParams::v1_default(1)
            },
            VolumeParams {
                detail_amp: -0.5,
                ..VolumeParams::v1_default(1)
            },
            VolumeParams {
                float_min: 1.5,
                ..VolumeParams::v1_default(1)
            },
            VolumeParams {
                sea: 0,
                ..VolumeParams::v1_default(1)
            },
        ];
        for params in cases {
            let error = params.validate().expect_err("invalid params are rejected");
            assert_eq!(error.stage, Stage::Volume);
            assert!(matches!(error.kind, ErrorKind::DescriptionInvalid { .. }));
            assert!(Volume::new(params).is_err());
        }
    }

    #[test]
    fn tunnel_radius_at_least_three_blocks() {
        // A tunnel axis is a cell where both spaghetti fields are near neutral, so
        // `tunnel` is a local maximum in all six directions. The six chord half-widths
        // average out the chord geometry of an oblique tunnel; the median over sampled
        // axes is the design radius. A level set can pinch, so the median, not the
        // worst single cell, is what `F2.1` promises.
        let volume = volume(0x5EED);
        let threshold = volume.params().cave_t;
        let top = crate::description::MAX_TERRAIN_Y;
        let mut means = Vec::new();

        for z in (0..192).step_by(2) {
            for x in (0..192).step_by(2) {
                let mut y = (SPAGHETTI_START_Y + 32) as f32;
                while y < (SPAGHETTI_FLAT_END_Y - 32) as f32 {
                    #[allow(clippy::cast_precision_loss)]
                    let (fx, fz) = (x as f32, z as f32);
                    if is_tunnel_axis(&volume, fx, y, fz, top) {
                        let mean = tunnel_radius(&volume, fx, y, fz, top, threshold);
                        means.push(mean);

                        if means.len() == 32 {
                            break;
                        }
                    }
                    y += 2.0;
                }
                if means.len() == 32 {
                    break;
                }
            }
            if means.len() == 32 {
                break;
            }
        }
        assert!(means.len() >= 8, "found only {} tunnel axes", means.len());
        means.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let lower_quartile = means[means.len() / 4];
        let median = means[means.len() / 2];
        assert!(
            median >= 3.0,
            "median tunnel radius is {median} blocks: {means:?}"
        );
        assert!(
            lower_quartile >= 3.0,
            "a quarter of tunnels are under three blocks: {means:?}"
        );
        assert!(
            *means.last().expect("non-empty") >= 5.0,
            "no wide tunnel in the sample: {means:?}"
        );
    }

    /// Whether `(x, y, z)` is a spaghetti tunnel axis.
    fn is_tunnel_axis(volume: &Volume, x: f32, y: f32, z: f32, top: i32) -> bool {
        let here = volume.spaghetti(x, y, z, top);
        here > 0.9
            && here > volume.spaghetti(x + 0.5, y, z, top)
            && here > volume.spaghetti(x - 0.5, y, z, top)
            && here > volume.spaghetti(x, y + 0.5, z, top)
            && here > volume.spaghetti(x, y - 0.5, z, top)
            && here > volume.spaghetti(x, y, z + 0.5, top)
            && here > volume.spaghetti(x, y, z - 0.5, top)
    }

    /// Mean half-width of the tunnel through `(x, y, z)` along the six axes.
    fn tunnel_radius(volume: &Volume, x: f32, y: f32, z: f32, top: i32, threshold: f32) -> f32 {
        let mut sum = 0.0;
        for (dx, dy, dz) in [
            (1.0, 0.0, 0.0),
            (-1.0, 0.0, 0.0),
            (0.0, 1.0, 0.0),
            (0.0, -1.0, 0.0),
            (0.0, 0.0, 1.0),
            (0.0, 0.0, -1.0),
        ] {
            sum += walk_to_edge(volume, x, y, z, dx, dy, dz, top, threshold);
        }
        sum / 6.0
    }

    /// Distance from `(x, y, z)` to the first cell at or below `threshold`.
    #[allow(clippy::too_many_arguments)]
    fn walk_to_edge(
        volume: &Volume,
        x: f32,
        y: f32,
        z: f32,
        dx: f32,
        dy: f32,
        dz: f32,
        top: i32,
        threshold: f32,
    ) -> f32 {
        let mut step = 1.0_f32;
        while step <= 64.0 {
            if volume.spaghetti(x + dx * step, y + dy * step, z + dz * step, top) <= threshold {
                return step - 1.0;
            }
            step += 1.0;
        }
        64.0
    }
}
