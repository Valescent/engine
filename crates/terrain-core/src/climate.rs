//! W4c - climate fields and nearest-point biome selection on the 4-block lattice.
//!
//! `E1.1`, `E1.2`, `E2.1`-`E2.3`, `E3.1`, `E3.3`, `E3.5`. One tile of temperature,
//! humidity, and biome picks is a pure function of the macro height grid, the biome
//! table, the climate parameters, and the seed.
//!
//! # The lattice is the boundary
//!
//! Climate never sees a per-block height. [`MacroCells`] is the only height channel and
//! it is cell-resolution, so a one-block bump or a river carve has nowhere to enter
//! (`E1.2`, `E2.3`). Height, snow line, and the biome arrays therefore agree for every
//! block of a column; no block can be one climate and its neighbour another.
//!
//! # Fields
//!
//! `temp = clamp(lat(|z - equator| / half_span) + noise - alt, 0, 1)`, where `lat` is a
//! caller-supplied monotone [`Curve`] that is warm at 0 and cold at 1, `noise` is a
//! three-octave fBm at 1/512 cycles per block scaled by [`TEMP_NOISE_AMP`], and
//! `alt = max(h_macro - SEA, 0) * lapse` (`E1.1`, `E1.2`). Latitude and the noise are
//! sampled at the cell's base block coordinate, so a cell mirrors with its counterpart
//! across the equator.
//!
//! The noise term is the interview's `(fbm - 0.5) * amplitude`, so it is asymmetric: it
//! spans `-0.225..=0.075` at the default amplitude, not `±0.075`. Latitude curves leave
//! headroom for that, and the clamp counter reports any world that saturates anyway.
//!
//! Humidity order is pinned (`E2.4`): `n = unit(noise)`, `d = 1 - strength * shadow`,
//! `t = n * d`, then `humidity = clamp(t + maritime * coast, 0, 1)`. The multiply
//! happens before the coast term is added, and there is exactly one clamp. With
//! `shadow = 0` and `coast = 0` that reduces to `n` bit-exactly, which a test pins by
//! recomputing the field from the same seed. Swapping the order (adding maritime
//! before multiplying) changes any column where both terms are live.
//!
//! # Rain shadow
//!
//! A fixed march toward `-x` (`WIND_DIR = X`, `E2.1`) reads [`SHADOW_RANGE_CELLS`] cells
//! upwind. `shadow = max_k(h[x-k] - h[x] - SLOPE_K * k * MACRO_CELL)`, floored at zero,
//! then `curve_shadow(shadow / SHADOW_NORM)` (`E2.2`). The march also records whether an
//! ocean cell (`h <= SEA`) sits upwind, which is the maritime coast term. The input grid
//! therefore carries [`SHADOW_RANGE_CELLS`] extra cells upwind (`E2.3`).
//!
//! # Biome selection
//!
//! Nearest point in `(temp, humidity)`: `d2 = (dt * T_SCALE)^2 + (dh * H_SCALE)^2 /
//! weight`, no square root, and argmin ties break by the lower id (`E3.1`). Only the top
//! two are kept. The margin `m = d2_second - d2_best` is converted to a block distance
//! by dividing by its own block-space gradient, then quantised once to a `u8`:
//! `q = clamp(floor(d / blend_width * 255 + 0.5), 0, 255)`, where
//! `d = m / |grad m|` (`E3.3`, `E3.5`). The gradient is analytic:
//! `grad m = 2 * [(T_b - T_s) grad T + ((H-H_s)/w_s - (H-H_b)/w_b) grad H]`, so it
//! needs no extra lattice reads and cannot split a border. Per block,
//! `w = 0.5 + 0.5 * smoothstep01(q / 255)`, and the pick is `best` when
//! `hash01(hash2(seed, dither_tag, x, z)) < w`, else `second`. The hash is keyed on
//! `(x, z)` only, never `y`.
//!
//! `grad T` and `grad H` are forward differences over one macro cell (four blocks),
//! taken from the tile's own fields. Forward differences are used because every global
//! cell then has one stencil, whether it is written as an interior cell or as the
//! neighbour's border cell; a central difference at the border would need a cell the
//! tile does not store.
//!
//! # What this module does not do
//!
//! No surface material, no snow decision, no terrain shape. `T_SNOW` (`E4.4`) and
//! `BLEND_BLOCKS` (`E3.3`) live in the description and are read by their own stages.
//! The v1 biome *set* is an open blocker (`E3.2`), so this module ships no table: the
//! table is data.
//!
//! # Determinism
//!
//! Every operation is `+ - * /`, `sqrt`-free, `pow`-free, and `mul_add`-free, in a fixed
//! order: the march is a fixed `1..=32` loop and the selection is one pass in table
//! order. No hash map, no float reduction of unfixed order. Stored fields are clamped
//! to `[0, 1]`; the clamp is a backstop and [`ClimateTile::clamp_fires`] counts every
//! time it fires.
//!
//! # Blend calibration
//!
//! The normalised `q` quantiser is pinned (`E3.5`). [`ClimateParams::blend_width`] is a
//! **calibration target**, not a magic number: the acceptance measurement is the
//! transect distance from `w = 0.5` to `w = 0.95`, doubled, with a median of 20-28
//! blocks, p10 >= 8, and p90 <= 64 per biome pair (`E3.6`). Because the measured
//! distance is `2 * s95 * blend_width`, where `s95 ≈ 0.8042` is the smoothstep argument
//! at `w = 0.95`, [`ClimateParams::v1_default`] derives the ramp from the description's
//! `BLEND_BLOCKS` target: `blend_width = BLEND_BLOCKS / (2 * s95)`. A straight border
//! therefore measures exactly `BLEND_BLOCKS`.
//!
//! # Open questions
//!
//! - `OPEN(E3.2)` the v1 biome set. This file ships no table and no default ids.

use std::cmp::Ordering;

use crate::curve::Curve;
use crate::description::{Axis, MACRO_CELL, SEA};
use crate::error::{ErrorKind, Stage, TerrainError};
use crate::hash::{domain, hash01, hash2};
use crate::noise::{fbm_2d, unit};

/// Cells stored per tile axis, border included (`E3.5`, `B4.2`).
///
/// A tile owns 32 cells (128 blocks) per axis. Cells `-1..=32` are readable through the
/// [`ClimateTile`] accessors; the remaining storage on each axis keeps the stride at
/// [`SIDE`] without a second index formula.
pub const SIDE: i32 = 35;

/// First readable cell coordinate: the one-cell border on the low side (`E3.5`).
pub const MIN_CELL: i32 = -1;

/// Last readable cell coordinate: the one-cell border on the high side (`E3.5`).
pub const MAX_CELL: i32 = 32;

/// Cells in one stored axis, as a `usize` stride.
#[allow(clippy::cast_sign_loss)]
pub const CELL_COUNT: usize = SIDE as usize * SIDE as usize;

/// Values in one padded macro grid: `(SIDE + SHADOW_RANGE_CELLS) * SIDE` (`E2.3`).
#[allow(clippy::cast_sign_loss)]
pub const PADDED_CELLS: usize = ((SIDE + SHADOW_RANGE_CELLS) * SIDE) as usize;

/// Amplitude of the temperature noise, in temperature units (`E1.1`).
pub const TEMP_NOISE_AMP: f32 = 0.15;

/// Distance the rain shadow march reaches upwind, in cells (`E2.2`).
pub const SHADOW_RANGE_CELLS: i32 = 32;

/// Slope the march subtracts per cell, in blocks per block (`E2.2`).
pub const SLOPE_K: f32 = 0.5;

/// Shadow depth that fills the shadow curve (`E2.2`).
pub const SHADOW_NORM: f32 = 32.0;

/// Humidity added on a windward coast (`E2.2`).
pub const MARITIME: f32 = 0.25;

/// Weight of the shadow curve in the drying multiplier (`E1.3`).
///
/// `E1.3` leaves `strength` unpinned. v1 folds it into the caller's shadow curve at 1.0,
/// so the curve alone owns the drying profile and retuning it is a description change,
/// not a code change.
pub const SHADOW_STRENGTH: f32 = 1.0;

/// Scale applied to a temperature delta before squaring (`E3.1`).
///
/// Both climate axes are `[0, 1]`, so the neutral scale keeps climate space a unit
/// square and a biome centre is directly comparable to a coordinate.
pub const T_SCALE: f32 = 1.0;

/// Scale applied to a humidity delta before squaring (`E3.1`).
pub const H_SCALE: f32 = 1.0;

/// The smoothstep argument at which the dither reaches `w = 0.95`.
///
/// `0.5 + 0.5 * smoothstep01(s) = 0.95`, so `s` solves `3s^2 - 2s^3 = 0.9`.
/// A straight border therefore measures `2 * S95 * blend_width` blocks.
const S95: f32 = 0.804_199_9;

/// The ramp length, in blocks, that measures as `blend_blocks` on a straight border.
///
/// The calibration measurement doubles the `w = 0.5` to `w = 0.95` distance, so the
/// quantiser's full ramp must be `blend_blocks / (2 * S95)` blocks long (`E3.5`,
/// `E3.6`).
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn blend_ramp_blocks(blend_blocks: u32) -> f32 {
    blend_blocks as f32 / (2.0 * S95)
}

/// The prevailing wind axis. Pinned to `X`: wind blows toward `+x`, the march goes
/// toward `-x` (`E2.1`).
pub const WIND_DIR: Axis = Axis::X;

/// Domain tag of the temperature noise stream.
pub const TEMP_DOMAIN: &str = "climate_temp";

/// Domain tag of the humidity noise stream.
pub const HUMIDITY_DOMAIN: &str = "climate_humidity";

/// Domain tag of the per-block biome dither (`E3.3`).
pub const DITHER_DOMAIN: &str = "biome_dither";

/// Frequency of the temperature noise, in cycles per block (`E1.1`).
const TEMP_NOISE_FREQ: f32 = 1.0 / 512.0;

/// Octaves of the temperature noise (`E1.1`).
const TEMP_NOISE_OCTAVES: u32 = 3;

/// Frequency of the humidity noise, in cycles per block.
const HUMIDITY_NOISE_FREQ: f32 = 1.0 / 512.0;

/// Octaves of the humidity noise.
const HUMIDITY_NOISE_OCTAVES: u32 = 3;

/// Lacunarity between noise octaves.
const NOISE_LACUNARITY: f32 = 2.0;

/// Gain between noise octaves.
const NOISE_GAIN: f32 = 0.5;

/// Sea level in blocks, as `f32` (`C1.2`).
#[allow(clippy::cast_precision_loss)]
const SEA_HEIGHT: f32 = SEA as f32;

/// One macro cell in blocks, as `f32` (`B4.1`).
#[allow(clippy::cast_precision_loss)]
const MACRO_CELL_F: f32 = MACRO_CELL as f32;

/// Index of one readable cell in a [`ClimateTile`] array (`E3.5`).
///
/// `(cell_x + 1) + SIDE * (cell_z + 1)`. The domain is `-1..=32` on both axes; the
/// accessors assert it before calling.
#[must_use]
pub fn index(cell_x: i32, cell_z: i32) -> usize {
    usize::try_from((cell_x + 1) + SIDE * (cell_z + 1))
        .expect("a climate cell index inside -1..=32 is non-negative")
}

/// Index of one cell in a [`MacroCells`] value array, upwind padding included (`E2.3`).
///
/// `x` runs from `-(SHADOW_RANGE_CELLS + 1)` to `SIDE - 2`, so the march can read
/// [`SHADOW_RANGE_CELLS`] cells upwind of the low border cell. `z` runs from `-1` to
/// `SIDE - 2`.
///
/// # Panics
///
/// Panics outside that padded range; a caller that pads a different grid must not use
/// this helper.
#[must_use]
pub fn padded_index(cell_x: i32, cell_z: i32) -> usize {
    assert!(
        (-(SHADOW_RANGE_CELLS + 1)..=(SIDE - 2)).contains(&cell_x),
        "macro cell x {cell_x} is outside the padded range"
    );
    assert!(
        (-1..=(SIDE - 2)).contains(&cell_z),
        "macro cell z {cell_z} is outside the padded range"
    );
    let x = cell_x + SHADOW_RANGE_CELLS + 1;
    let z = cell_z + 1;
    usize::try_from(x + (SIDE + SHADOW_RANGE_CELLS) * z)
        .expect("a checked padded index is non-negative")
}

/// The padded pre-detail macro height grid for one tile (`E2.3`).
///
/// The grid is `width = SIDE + SHADOW_RANGE_CELLS` cells wide and `depth = SIDE` cells
/// deep, with [`SHADOW_RANGE_CELLS`] extra cells on the upwind (`-x`) side. Heights are
/// in blocks, before detail, rivers, or caves; the climate stage reads nothing else.
#[derive(Clone, Debug)]
pub struct MacroCells {
    /// Cells per row, `SIDE + SHADOW_RANGE_CELLS`.
    pub width: i32,
    /// Cells per column, `SIDE`.
    pub depth: i32,
    /// Row-major heights in blocks.
    values: Vec<f32>,
}

impl MacroCells {
    /// Wraps a padded height grid, checking its shape and finiteness.
    ///
    /// # Errors
    ///
    /// Rejects a width or depth other than [`SIDE`] plus [`SHADOW_RANGE_CELLS`], a
    /// value count that does not match, and any non-finite height (`S1.9` rejects,
    /// never clamps).
    pub fn new(width: i32, depth: i32, values: Box<[f32]>) -> Result<Self, TerrainError> {
        if width != SIDE + SHADOW_RANGE_CELLS || depth != SIDE {
            return Err(TerrainError::new(
                Stage::Fields,
                ErrorKind::InvariantViolated {
                    check: "macro_cells_shape",
                    coord: None,
                },
            ));
        }
        if values.len() != PADDED_CELLS {
            return Err(TerrainError::new(
                Stage::Fields,
                ErrorKind::InvariantViolated {
                    check: "macro_cells_len",
                    coord: None,
                },
            ));
        }
        if values.iter().any(|height| !height.is_finite()) {
            return Err(TerrainError::new(
                Stage::Fields,
                ErrorKind::InvariantViolated {
                    check: "macro_cells_finite",
                    coord: None,
                },
            ));
        }
        Ok(Self {
            width,
            depth,
            values: Vec::from(values),
        })
    }

    /// Height in blocks at one padded cell (`E2.3`).
    ///
    /// # Panics
    ///
    /// Panics outside the padded range of [`padded_index`].
    #[must_use]
    pub fn get(&self, cell_x: i32, cell_z: i32) -> f32 {
        debug_assert_eq!(self.width, SIDE + SHADOW_RANGE_CELLS);
        debug_assert_eq!(self.depth, SIDE);
        self.values[padded_index(cell_x, cell_z)]
    }
}

/// The result of the fixed rain-shadow march (`E2.2`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    /// Raw shadow depth in blocks, `>= 0`, before the shadow curve.
    pub depth: f32,
    /// Whether an ocean cell (`h <= SEA`) sits upwind inside the march.
    pub ocean_upwind: bool,
}

/// Marches [`SHADOW_RANGE_CELLS`] cells toward `-x` from one cell (`E2.1`, `E2.2`).
///
/// `depth = max over k in 1..=32 of (h[x-k] - h[x] - SLOPE_K * k * MACRO_CELL)`, floored
/// at zero. The loop order, the operand order, and the step are fixed, so the value is
/// bit-stable. `ocean_upwind` records the first ocean cell the same march meets.
///
/// # Panics
///
/// Panics when the cell is outside the readable climate range or the grid lacks the
/// upwind padding.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn shadow(macro_cells: &MacroCells, cell_x: i32, cell_z: i32) -> Shadow {
    let here = macro_cells.get(cell_x, cell_z);
    let mut depth = 0.0_f32;
    let mut ocean_upwind = false;
    for k in 1..=SHADOW_RANGE_CELLS {
        let upwind = macro_cells.get(cell_x - k, cell_z);
        let rise = upwind - here - SLOPE_K * k as f32 * MACRO_CELL_F;
        if rise > depth {
            depth = rise;
        }
        if !ocean_upwind && upwind <= SEA_HEIGHT {
            ocean_upwind = true;
        }
    }
    Shadow {
        depth,
        ocean_upwind,
    }
}

/// The three-way smoothstep on `[0, 1]`: `t * t * (3 - 2t)` (`B2.3`, `E3.5`).
#[must_use]
pub fn smoothstep01(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// One biome centre in climate space (`E3.1`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Biome {
    /// Stable id stored in the palette (`E3.1`).
    pub id: u8,
    /// Temperature centre in `[0, 1]`.
    pub temp_c: f32,
    /// Humidity centre in `[0, 1]`.
    pub humid_c: f32,
    /// Weight of the humidity axis; positive.
    pub weight: f32,
    /// Ordering priority carried for the content pack. Not read by the nearest-point
    /// rule; non-climate overrides apply later (`E3.1`).
    pub priority: u8,
}

/// A validated biome table. The table is data: no v1 set is pinned (`E3.2`).
#[derive(Clone, Debug)]
pub struct BiomeTable {
    /// Biomes in declaration order.
    biomes: Vec<Biome>,
}

/// Folds `-0.0` into `+0.0` so duplicate detection matches the description hash
/// (`A1.1`).
fn centre_key(value: f32) -> u32 {
    value.to_bits() & 0x7fff_ffff
}

/// Squared distance in climate space (`E3.1`). No square root.
fn distance2(temperature: f32, humidity: f32, biome: &Biome) -> f32 {
    let dt = (temperature - biome.temp_c) * T_SCALE;
    let dh = (humidity - biome.humid_c) * H_SCALE;
    dt.powi(2) + dh.powi(2) / biome.weight
}

fn invalid(key: &'static str, rule: &'static str) -> TerrainError {
    TerrainError::new(
        Stage::Description,
        ErrorKind::DescriptionInvalid { key, rule },
    )
}

impl BiomeTable {
    /// Validates and stores a biome table.
    ///
    /// # Errors
    ///
    /// Rejects an empty table, more than 255 entries, duplicated ids, duplicated
    /// centres, a centre outside `[0, 1]`, a non-finite value, and `weight <= 0`. Every
    /// rejection is `ErrorKind::DescriptionInvalid` with key `BIOMES`.
    pub fn new(biomes: Vec<Biome>) -> Result<Self, TerrainError> {
        if biomes.is_empty() {
            return Err(invalid("BIOMES", "non-empty"));
        }
        if biomes.len() > 255 {
            return Err(invalid("BIOMES", "at most 255 entries"));
        }
        for (position, biome) in biomes.iter().enumerate() {
            if !biome.temp_c.is_finite() || !(0.0..=1.0).contains(&biome.temp_c) {
                return Err(invalid("BIOMES", "temperature centre range"));
            }
            if !biome.humid_c.is_finite() || !(0.0..=1.0).contains(&biome.humid_c) {
                return Err(invalid("BIOMES", "humidity centre range"));
            }
            if !biome.weight.is_finite() || biome.weight <= 0.0 {
                return Err(invalid("BIOMES", "weight positive"));
            }
            for earlier in &biomes[..position] {
                if earlier.id == biome.id {
                    return Err(invalid("BIOMES", "duplicate id"));
                }
                if centre_key(earlier.temp_c) == centre_key(biome.temp_c)
                    && centre_key(earlier.humid_c) == centre_key(biome.humid_c)
                {
                    return Err(invalid("BIOMES", "duplicate centre"));
                }
            }
        }
        Ok(Self { biomes })
    }

    /// The two nearest biome slots and the raw `d2` margin between them (`E3.1`).
    ///
    /// Ties break by the lower id. A single-biome table returns the same slot
    /// twice and a zero margin.
    fn nearest_slots(&self, temperature: f32, humidity: f32) -> (usize, usize, f32) {
        let mut best_slot = 0_usize;
        let mut best_d2 = f32::INFINITY;
        let mut best_id = u8::MAX;
        for (slot, biome) in self.biomes.iter().enumerate() {
            let d2 = distance2(temperature, humidity, biome);
            match d2.total_cmp(&best_d2) {
                Ordering::Less => {
                    best_d2 = d2;
                    best_id = biome.id;
                    best_slot = slot;
                }
                Ordering::Equal if biome.id < best_id => {
                    best_id = biome.id;
                    best_slot = slot;
                }
                _ => {}
            }
        }

        if self.biomes.len() == 1 {
            return (best_slot, best_slot, 0.0);
        }

        let mut second_slot = 0_usize;
        let mut second_d2 = f32::INFINITY;
        let mut second_id = u8::MAX;
        for (slot, biome) in self.biomes.iter().enumerate() {
            if slot == best_slot {
                continue;
            }
            let d2 = distance2(temperature, humidity, biome);
            match d2.total_cmp(&second_d2) {
                Ordering::Less => {
                    second_d2 = d2;
                    second_id = biome.id;
                    second_slot = slot;
                }
                Ordering::Equal if biome.id < second_id => {
                    second_id = biome.id;
                    second_slot = slot;
                }
                _ => {}
            }
        }

        (best_slot, second_slot, (second_d2 - best_d2).max(0.0))
    }

    /// The two nearest biomes and the raw `d2` margin quantised directly
    /// (`E3.1`, `E3.3`).
    ///
    /// Returns `(best, second, q)`. Ties break by the lower id. `q = 0` is the border
    /// between the two (`w = 0.5`) and `q = 255` is deep inside `best` (`w = 1`). A
    /// single-biome table returns `(id, id, 255)`.
    ///
    /// `blend_width` is the margin, in `d2` units, at which the dither reaches certainty.
    /// This is the raw quantiser; generation uses [`Self::nearest_two_normalised`] so a
    /// pair's transition is a fixed number of blocks rather than a fixed `d2` margin.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn nearest_two(&self, temperature: f32, humidity: f32, blend_width: f32) -> (u8, u8, u8) {
        let (best_slot, second_slot, margin) = self.nearest_slots(temperature, humidity);
        let best = &self.biomes[best_slot];
        if self.biomes.len() == 1 {
            return (best.id, best.id, 255);
        }
        let second = &self.biomes[second_slot];
        let scaled = margin / blend_width * 255.0 + 0.5;
        let quantised = scaled.floor().clamp(0.0, 255.0);
        (best.id, second.id, quantised as u8)
    }

    /// The two nearest biomes and the quantised **block distance** to their border
    /// (`E3.5`).
    ///
    /// `temperature_gradient` and `humidity_gradient` are `(d/dx, d/dz)` in climate
    /// units per block. The `d2` margin is divided by its own analytic gradient,
    /// `grad m = 2 * [(T_b - T_s) grad T + ((H - H_s)/w_s - (H - H_b)/w_b) grad H]`,
    /// which is the straight-line distance to the border in blocks. `blend_width` is
    /// the ramp length, in blocks, at which the dither reaches certainty. A locally
    /// flat field with a positive margin is infinitely far from the border, so it
    /// quantises to `255`; a flat tie is `0`.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn nearest_two_normalised(
        &self,
        temperature: f32,
        humidity: f32,
        temperature_gradient: (f32, f32),
        humidity_gradient: (f32, f32),
        blend_width: f32,
    ) -> (u8, u8, u8) {
        let (best_slot, second_slot, margin) = self.nearest_slots(temperature, humidity);
        let best = &self.biomes[best_slot];
        if self.biomes.len() == 1 {
            return (best.id, best.id, 255);
        }
        let second = &self.biomes[second_slot];
        let temp_delta = best.temp_c - second.temp_c;
        let humid_delta =
            (humidity - second.humid_c) / second.weight - (humidity - best.humid_c) / best.weight;
        let gradient_x =
            2.0 * (temp_delta * temperature_gradient.0 + humid_delta * humidity_gradient.0);
        let gradient_z =
            2.0 * (temp_delta * temperature_gradient.1 + humid_delta * humidity_gradient.1);
        let magnitude = (gradient_x * gradient_x + gradient_z * gradient_z).sqrt();
        let distance = if magnitude > 0.0 {
            margin / magnitude
        } else if margin > 0.0 {
            f32::INFINITY
        } else {
            0.0
        };
        let scaled = distance / blend_width * 255.0 + 0.5;
        let quantised = scaled.floor().clamp(0.0, 255.0);
        (best.id, second.id, quantised as u8)
    }
}

/// Per-world climate parameters (`E1.1`, `E1.2`, `E3.3`).
///
/// Every field is a constructor value rather than a constant because the description
/// owns them (`A1.3`). The two curves are compiled once and shared by every tile.
#[derive(Clone, Debug)]
pub struct ClimateParams {
    /// World `z` of the equator, in blocks (`E1.1`).
    pub equator: i32,
    /// Distance from the equator to the cold pole, in blocks (`E1.1`).
    pub half_span: i32,
    /// Temperature drop per block above sea level (`E1.2`).
    pub lapse: f32,
    /// Normalised block distance at which the biome dither reaches certainty
    /// (`E3.3`).
    pub blend_width: f32,
    /// Latitude curve: warm at 0, cold at 1 (`E1.1`).
    pub latitude: Curve,
    /// Shadow curve: `0` at no shadow, `1` at full drying (`E2.2`).
    pub shadow: Curve,
}

impl ClimateParams {
    /// The v1 defaults, matching [`crate::description::default_v1`] where they overlap.
    ///
    /// The latitude curve ends at 0.92 and 0.25 rather than 1 and 0: the noise term
    /// reaches 0.225 below its centre, so a curve that touched either end would saturate
    /// at the pole on flat ground.
    ///
    /// `blend_width` is the **calibration target** for the `E3.6` sweep: it is derived
    /// from the description's `BLEND_BLOCKS` so that a straight border measures exactly
    /// that many blocks. See [`blend_ramp_blocks`].
    #[must_use]
    pub fn v1_default() -> Self {
        Self {
            equator: 0,
            half_span: 1_024,
            lapse: 1.0 / 320.0,
            blend_width: blend_ramp_blocks(crate::description::default_v1().blend_blocks),
            latitude: Curve::new(&[(0.0, 0.92), (0.35, 0.74), (0.70, 0.40), (1.0, 0.25)])
                .expect("the v1 latitude knots are valid"),
            shadow: Curve::new(&[(0.0, 0.0), (0.35, 0.18), (0.75, 0.60), (1.0, 1.0)])
                .expect("the v1 shadow knots are valid"),
        }
    }

    /// Checks ranges and finiteness.
    ///
    /// # Errors
    ///
    /// Rejects a non-positive `half_span` and a non-finite or non-positive `lapse` or
    /// `blend_width`. The curves are validated by [`Curve::new`] when the parameters are
    /// built. Key `CLIMATE`.
    pub fn validate(&self) -> Result<(), TerrainError> {
        if self.half_span <= 0 {
            return Err(invalid("CLIMATE", "half span positive"));
        }
        if !self.lapse.is_finite() || self.lapse <= 0.0 {
            return Err(invalid("CLIMATE", "lapse finite positive"));
        }
        if !self.blend_width.is_finite() || self.blend_width <= 0.0 {
            return Err(invalid("CLIMATE", "blend width finite positive"));
        }
        Ok(())
    }
}

/// One tile of lattice climate and biome picks (`E3.5`).
///
/// Five parallel arrays of [`CELL_COUNT`] entries: temperature, humidity, and the three
/// `u8` biome arrays. Accessors accept cells `-1..=32` and panic outside it.
#[derive(Clone, Debug)]
pub struct ClimateTile {
    /// Temperature in `[0, 1]`.
    temp: Vec<f32>,
    /// Humidity in `[0, 1]`.
    humidity: Vec<f32>,
    /// Nearest biome id.
    best: Vec<u8>,
    /// Second-nearest biome id; equal to `best` in a single-biome world.
    second: Vec<u8>,
    /// Quantised block distance to the border in `0..=255` (`E3.3`, `E3.5`).
    margin: Vec<u8>,
    /// How many times a field clamp fired while building the tile.
    clamp_fires: u64,
}

/// Checks the readable cell range and returns the array slot.
fn slot(cell_x: i32, cell_z: i32) -> usize {
    assert!(
        (MIN_CELL..=MAX_CELL).contains(&cell_x) && (MIN_CELL..=MAX_CELL).contains(&cell_z),
        "climate cell ({cell_x}, {cell_z}) is outside {MIN_CELL}..={MAX_CELL}"
    );
    index(cell_x, cell_z)
}

impl ClimateTile {
    /// Temperature at one cell, in `[0, 1]` (`E1.1`).
    #[must_use]
    pub fn temp(&self, cell_x: i32, cell_z: i32) -> f32 {
        self.temp[slot(cell_x, cell_z)]
    }

    /// Humidity at one cell, in `[0, 1]` (`E1.3`).
    #[must_use]
    pub fn humidity(&self, cell_x: i32, cell_z: i32) -> f32 {
        self.humidity[slot(cell_x, cell_z)]
    }

    /// Nearest biome id at one cell (`E3.1`).
    #[must_use]
    pub fn best(&self, cell_x: i32, cell_z: i32) -> u8 {
        self.best[slot(cell_x, cell_z)]
    }

    /// Second-nearest biome id at one cell (`E3.1`).
    #[must_use]
    pub fn second(&self, cell_x: i32, cell_z: i32) -> u8 {
        self.second[slot(cell_x, cell_z)]
    }

    /// Quantised margin at one cell, `0` on the border and `255` deep inside (`E3.3`).
    #[must_use]
    pub fn margin(&self, cell_x: i32, cell_z: i32) -> u8 {
        self.margin[slot(cell_x, cell_z)]
    }

    /// How many times a stored field was clamped to `[0, 1]` while building this tile.
    ///
    /// The clamp is a backstop: a fixture with headroom reports zero. A tile that
    /// reports a non-zero count has saturated at a pole or under a tall peak, which is
    /// legal but worth a debug view.
    #[must_use]
    pub fn clamp_fires(&self) -> u64 {
        self.clamp_fires
    }

    /// Resolves the per-block biome pick (`E3.3`, `E3.5`).
    ///
    /// Reconstructs `w = 0.5 + 0.5 * smoothstep01(q / 255)` from the cell's margin and
    /// returns `best` when `hash01(hash2(seed, dither_tag, block_x, block_z)) < w`, else
    /// `second`. The hash is keyed on `(x, z)` only, so a column has one biome.
    #[must_use]
    pub fn biome_pick(
        &self,
        cell_x: i32,
        cell_z: i32,
        block_x: i32,
        block_z: i32,
        seed: u64,
        dither_tag: u64,
    ) -> u8 {
        let slot = slot(cell_x, cell_z);
        let q = f32::from(self.margin[slot]) / 255.0;
        let weight = 0.5 + 0.5 * smoothstep01(q);
        if hash01(hash2(seed, dither_tag, block_x, block_z)) < weight {
            self.best[slot]
        } else {
            self.second[slot]
        }
    }
}

/// Clamps one stored field to `[0, 1]` and counts every fire.
fn clamp_unit(value: f32, fires: &mut u64) -> f32 {
    debug_assert!(value.is_finite(), "a climate field left the finite range");
    if value < 0.0 {
        *fires += 1;
        0.0
    } else if value > 1.0 {
        *fires += 1;
        1.0
    } else {
        value
    }
}

/// Temperature's latitude term alone: `lat(|z - equator| / half_span)` (`E1.1`).
///
/// Exposed for debug views and tests; the solver composes it with noise and altitude.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn latitude(params: &ClimateParams, block_z: i32) -> f32 {
    let offset = (i64::from(block_z) - i64::from(params.equator)).unsigned_abs();
    params
        .latitude
        .sample(offset as f32 / params.half_span as f32)
}

/// Temperature's altitude term alone: `max(h_macro - SEA, 0) * lapse` (`E1.2`).
///
/// No lapse below sea level, and the input is the pre-detail macro height only.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn altitude(params: &ClimateParams, macro_height: f32) -> f32 {
    (macro_height - SEA_HEIGHT).max(0.0) * params.lapse
}

/// World position of the lattice sample for one global cell (`B4.5`).
#[must_use]
#[allow(clippy::cast_precision_loss)]
fn sample_position(global_cell: i32) -> f32 {
    (global_cell * MACRO_CELL) as f32 + 0.5
}

/// Derives the `u32` noise seed for one 64-bit stream.
fn stream_seed(seed: u64, tag: u64) -> u32 {
    let bytes = hash2(seed, tag, 0, 0).to_le_bytes();
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// The climate stage. Stateless apart from its cached stream tags.
#[derive(Clone, Copy, Debug)]
pub struct Climate {
    /// Domain tag of the temperature noise stream.
    temp_tag: u64,
    /// Domain tag of the humidity noise stream.
    humid_tag: u64,
}

impl Default for Climate {
    fn default() -> Self {
        Self::new()
    }
}

impl Climate {
    /// Builds the stage, hashing the two noise domain tags once.
    #[must_use]
    pub fn new() -> Self {
        Self {
            temp_tag: domain(TEMP_DOMAIN),
            humid_tag: domain(HUMIDITY_DOMAIN),
        }
    }

    /// Solves the tile whose local cell `0` is global cell `0`.
    ///
    /// Equivalent to [`Climate::solve_at`] with a zero origin.
    #[must_use]
    pub fn solve(
        &self,
        macro_cells: &MacroCells,
        table: &BiomeTable,
        params: &ClimateParams,
        seed: u64,
    ) -> ClimateTile {
        self.solve_at(0, 0, macro_cells, table, params, seed)
    }

    /// Solves the tile whose local cell `0` is global cell `(origin_x, origin_z)`.
    ///
    /// The origin is in cells. It moves the latitude and the noise sample; the macro
    /// grid stays tile-relative, so the same global field yields bit-identical border
    /// cells in neighbouring tiles. The fields are written for `-1..=33` and the biome
    /// arrays for the readable `-1..=32`; no per-cell allocation happens.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn solve_at(
        &self,
        origin_x: i32,
        origin_z: i32,
        macro_cells: &MacroCells,
        table: &BiomeTable,
        params: &ClimateParams,
        seed: u64,
    ) -> ClimateTile {
        debug_assert!(params.half_span > 0, "half span must be positive");
        debug_assert!(params.blend_width > 0.0, "blend width must be positive");
        debug_assert!(matches!(WIND_DIR, Axis::X), "the march is pinned to -x");

        let temp_seed = stream_seed(seed, self.temp_tag);
        let humid_seed = stream_seed(seed, self.humid_tag);
        let mut temp = vec![0.0_f32; CELL_COUNT];
        let mut humidity = vec![0.0_f32; CELL_COUNT];
        let mut best = vec![0_u8; CELL_COUNT];
        let mut second = vec![0_u8; CELL_COUNT];
        let mut margin = vec![0_u8; CELL_COUNT];
        let mut clamp_fires = 0_u64;

        // Pass 1: the two fields, over the readable cells plus one extra cell on
        // each high side. The extra cell (`SIDE - 2`, i.e. 33) is what lets every
        // readable cell take a *forward* difference.
        for cell_z in MIN_CELL..=(SIDE - 2) {
            for cell_x in MIN_CELL..=(SIDE - 2) {
                let slot = index(cell_x, cell_z);
                let block_z = (origin_z + cell_z) * MACRO_CELL;
                let sample_x = sample_position(origin_x + cell_x);
                let sample_z = sample_position(origin_z + cell_z);

                let lat = latitude(params, block_z);
                let noise = (fbm_2d(
                    temp_seed,
                    sample_x,
                    sample_z,
                    TEMP_NOISE_FREQ,
                    TEMP_NOISE_OCTAVES,
                    NOISE_LACUNARITY,
                    NOISE_GAIN,
                ) - 0.5)
                    * TEMP_NOISE_AMP;
                let height = macro_cells.get(cell_x, cell_z);
                let alt = altitude(params, height);
                let temperature = clamp_unit(lat + noise - alt, &mut clamp_fires);
                temp[slot] = temperature;

                let march = shadow(macro_cells, cell_x, cell_z);
                let drying = params.shadow.sample(march.depth / SHADOW_NORM);
                let wet = fbm_2d(
                    humid_seed,
                    sample_x,
                    sample_z,
                    HUMIDITY_NOISE_FREQ,
                    HUMIDITY_NOISE_OCTAVES,
                    NOISE_LACUNARITY,
                    NOISE_GAIN,
                );
                let coast = if march.ocean_upwind { 1.0 } else { 0.0 };
                let value = unit(wet) * (1.0 - SHADOW_STRENGTH * drying) + MARITIME * coast;
                let moisture = clamp_unit(value, &mut clamp_fires);
                humidity[slot] = moisture;
            }
        }

        // Pass 2: the biome arrays. A forward difference keeps one stencil per
        // *global* cell, so a border cell written by one tile and the same cell
        // seen as interior by its neighbour agree bit for bit.
        for cell_z in MIN_CELL..=MAX_CELL {
            for cell_x in MIN_CELL..=MAX_CELL {
                let slot = index(cell_x, cell_z);
                let ahead_x = index(cell_x + 1, cell_z);
                let ahead_z = index(cell_x, cell_z + 1);
                let temperature_gradient = (
                    (temp[ahead_x] - temp[slot]) / MACRO_CELL_F,
                    (temp[ahead_z] - temp[slot]) / MACRO_CELL_F,
                );
                let humidity_gradient = (
                    (humidity[ahead_x] - humidity[slot]) / MACRO_CELL_F,
                    (humidity[ahead_z] - humidity[slot]) / MACRO_CELL_F,
                );
                let (near, far, q) = table.nearest_two_normalised(
                    temp[slot],
                    humidity[slot],
                    temperature_gradient,
                    humidity_gradient,
                    params.blend_width,
                );
                best[slot] = near;
                second[slot] = far;
                margin[slot] = q;
            }
        }

        ClimateTile {
            temp,
            humidity,
            best,
            second,
            margin,
            clamp_fires,
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
mod tests {
    use super::*;

    const TILE_BLOCKS: i32 = 128;

    /// A fixture with a short half span, so one tile covers most of the latitude
    /// range, and straight curves, so closed forms are exact.
    fn test_params() -> ClimateParams {
        ClimateParams {
            equator: 0,
            half_span: 64,
            lapse: 1.0 / 320.0,
            blend_width: blend_ramp_blocks(24),
            latitude: Curve::linear(&[(0.0, 0.92), (1.0, 0.3)]).unwrap(),
            shadow: Curve::linear(&[(0.0, 0.0), (1.0, 1.0)]).unwrap(),
        }
    }

    /// Shape-only parameters with a world-sized half span. Every cell of one tile stays
    /// in the warm half, so a fixture with land above sea has clamp headroom.
    fn wide_params() -> ClimateParams {
        ClimateParams {
            half_span: 1_024,
            ..test_params()
        }
    }

    fn table_2x2() -> BiomeTable {
        BiomeTable::new(vec![
            Biome {
                id: 1,
                temp_c: 0.2,
                humid_c: 0.2,
                weight: 1.0,
                priority: 0,
            },
            Biome {
                id: 2,
                temp_c: 0.2,
                humid_c: 0.8,
                weight: 1.0,
                priority: 0,
            },
            Biome {
                id: 3,
                temp_c: 0.8,
                humid_c: 0.2,
                weight: 1.0,
                priority: 0,
            },
            Biome {
                id: 4,
                temp_c: 0.8,
                humid_c: 0.8,
                weight: 1.0,
                priority: 0,
            },
        ])
        .unwrap()
    }

    /// Builds a padded grid from a global height function, mapping tile-local cells to
    /// global cells through the origin.
    fn cells(origin_x: i32, origin_z: i32, height: impl Fn(i32, i32) -> f32) -> MacroCells {
        let mut values = vec![0.0_f32; PADDED_CELLS];
        for cell_z in -1..=(SIDE - 2) {
            for cell_x in -(SHADOW_RANGE_CELLS + 1)..=(SIDE - 2) {
                values[padded_index(cell_x, cell_z)] = height(origin_x + cell_x, origin_z + cell_z);
            }
        }
        MacroCells::new(SIDE + SHADOW_RANGE_CELLS, SIDE, values.into_boxed_slice()).unwrap()
    }

    #[test]
    fn temperature_falls_with_latitude() {
        let params = test_params();

        // The latitude term is exact: it falls from the equator to the half span and
        // is symmetric about the equator.
        let mut previous = f32::INFINITY;
        for block_z in 0..=params.half_span {
            let value = latitude(&params, block_z);
            assert!(value <= previous, "not monotone at {block_z}");
            assert_eq!(
                value.to_bits(),
                latitude(&params, -block_z).to_bits(),
                "not symmetric at {block_z}"
            );
            previous = value;
        }
        assert!(latitude(&params, 0) > latitude(&params, params.half_span));

        // The whole field follows it across one tile. The noise is bounded and its
        // slope is small next to the latitude slope at this span.
        let climate = Climate::new();
        let tile = climate.solve(
            &cells(0, 0, |_, _| SEA_HEIGHT + 2.0),
            &table_2x2(),
            &params,
            7,
        );
        let mut previous = f32::INFINITY;
        for cell_z in 0..=(params.half_span / MACRO_CELL) {
            let value = tile.temp(0, cell_z);
            assert!(value <= previous, "temperature rose at cell {cell_z}");
            previous = value;
        }
        assert!(tile.temp(0, 0) > tile.temp(0, params.half_span / MACRO_CELL));
        assert_eq!(tile.clamp_fires(), 0);
    }

    #[test]
    fn lapse_only_above_sea() {
        let params = wide_params();
        let climate = Climate::new();
        let table = table_2x2();
        let (cell_x, cell_z) = (4, 5);

        assert_eq!(altitude(&params, SEA_HEIGHT + 8.0), 8.0 * params.lapse);
        assert_eq!(altitude(&params, SEA_HEIGHT), 0.0);
        assert_eq!(altitude(&params, SEA_HEIGHT - 64.0), 0.0);

        // Land fixtures: no ocean upwind, so no coast term and no saturation.
        let seven_up = climate.solve(&cells(0, 0, |_, _| SEA_HEIGHT + 7.0), &table, &params, 3);
        let eight_up = climate.solve(&cells(0, 0, |_, _| SEA_HEIGHT + 8.0), &table, &params, 3);
        let seventeen_up =
            climate.solve(&cells(0, 0, |_, _| SEA_HEIGHT + 17.0), &table, &params, 3);
        let drop_one = seven_up.temp(cell_x, cell_z) - eight_up.temp(cell_x, cell_z);
        assert!((drop_one - params.lapse).abs() < 1e-6, "{drop_one}");
        let drop_ten = seven_up.temp(cell_x, cell_z) - seventeen_up.temp(cell_x, cell_z);
        assert!((drop_ten - 10.0 * params.lapse).abs() < 1e-6, "{drop_ten}");
        assert_eq!(seven_up.clamp_fires(), 0);
        assert_eq!(seventeen_up.clamp_fires(), 0);

        // At and below sea the altitude term is zero, so the temperature is identical.
        // Both fixtures are ocean by definition, which is why their clamp counters are
        // not asserted here.
        let at_sea = climate.solve(&cells(0, 0, |_, _| SEA_HEIGHT), &table, &params, 3);
        let below = climate.solve(&cells(0, 0, |_, _| SEA_HEIGHT - 40.0), &table, &params, 3);
        assert_eq!(
            below.temp(cell_x, cell_z).to_bits(),
            at_sea.temp(cell_x, cell_z).to_bits(),
            "no lapse below sea"
        );
    }

    #[test]
    fn climate_ignores_per_block_height() {
        // A per-block field, with a river carved into it and a one-block bump, is built
        // here and never consulted. `MacroCells` is the only height channel and it is
        // cell-resolution, so neither can reach the field.
        let smooth = vec![SEA; (TILE_BLOCKS * TILE_BLOCKS) as usize];
        let mut detailed = smooth.clone();
        for block_z in 0..TILE_BLOCKS {
            detailed[(block_z * TILE_BLOCKS + 60) as usize] -= 6;
        }
        detailed[(64 * TILE_BLOCKS + 64) as usize] += 1;
        assert_ne!(smooth, detailed);

        let params = wide_params();
        let climate = Climate::new();
        let table = table_2x2();
        let tile = climate.solve(&cells(0, 0, |_, _| SEA_HEIGHT + 10.0), &table, &params, 11);
        let repeat = climate.solve(&cells(0, 0, |_, _| SEA_HEIGHT + 10.0), &table, &params, 11);
        for cell_z in MIN_CELL..=MAX_CELL {
            for cell_x in MIN_CELL..=MAX_CELL {
                assert_eq!(
                    tile.temp(cell_x, cell_z).to_bits(),
                    repeat.temp(cell_x, cell_z).to_bits()
                );
                assert_eq!(
                    tile.humidity(cell_x, cell_z).to_bits(),
                    repeat.humidity(cell_x, cell_z).to_bits()
                );
                assert_eq!(tile.margin(cell_x, cell_z), repeat.margin(cell_x, cell_z));
            }
        }

        // A cell-resolution bump does move the field, so the comparison above is not
        // vacuous: the macro grid really is read.
        let bumped = cells(0, 0, |cell_x, cell_z| {
            if cell_x == 0 && cell_z == 0 {
                SEA_HEIGHT + 14.0
            } else {
                SEA_HEIGHT + 10.0
            }
        });
        let bumped_tile = climate.solve(&bumped, &table, &params, 11);
        assert_ne!(tile.temp(0, 0).to_bits(), bumped_tile.temp(0, 0).to_bits());
    }

    #[test]
    fn rain_shadow_is_lee_side_only() {
        // A single ridge at cell x = 24, land everywhere else. Wind blows toward +x, so
        // cells above the ridge are the lee and cells below it are windward.
        let params = wide_params();
        let climate = Climate::new();
        let table = table_2x2();
        let grid = cells(0, 0, |cell_x, _| {
            if cell_x == 24 {
                SEA_HEIGHT + 100.0
            } else {
                SEA_HEIGHT + 2.0
            }
        });
        let tile = climate.solve(&grid, &table, &params, 13);

        for cell_z in MIN_CELL..=MAX_CELL {
            for cell_x in 25..=32 {
                assert_eq!(
                    tile.humidity(cell_x, cell_z),
                    0.0,
                    "lee cell {cell_x} was not dried"
                );
            }
            for cell_x in 0..=23 {
                assert!(
                    tile.humidity(cell_x, cell_z) > 0.0,
                    "windward cell {cell_x} was dried"
                );
            }
        }
    }

    #[test]
    fn shadow_march_is_fixed_and_bounded() {
        // h(x) = SEA - r*x: every upwind cell rises by r per cell, so the closed form is
        // max_k (r - SLOPE_K*MACRO_CELL) * k, which is largest at k = 32 for r = 3.
        let rise = 3.0_f32;
        let rising = cells(0, 0, |cell_x, _| SEA_HEIGHT - rise * cell_x as f32);
        let step = SLOPE_K * MACRO_CELL_F;
        let expected = (rise - step) * SHADOW_RANGE_CELLS as f32;
        for cell_x in MIN_CELL..=MAX_CELL {
            assert_eq!(shadow(&rising, cell_x, 0).depth, expected, "at {cell_x}");
        }

        // A ramp exactly as steep as the slope term casts nothing; half again as steep
        // casts exactly half the range.
        let slack = cells(0, 0, |cell_x, _| SEA_HEIGHT - step * cell_x as f32);
        assert_eq!(shadow(&slack, 0, 0).depth, 0.0);
        let steeper = cells(0, 0, |cell_x, _| SEA_HEIGHT - (step + 0.5) * cell_x as f32);
        assert_eq!(
            shadow(&steeper, 0, 0).depth,
            0.5 * SHADOW_RANGE_CELLS as f32
        );

        // Ocean upwind is reported, land upwind is not.
        let coast = cells(0, 0, |cell_x, _| {
            if cell_x <= -8 {
                SEA_HEIGHT - 4.0
            } else {
                SEA_HEIGHT + 2.0
            }
        });
        assert!(shadow(&coast, 0, 0).ocean_upwind);
        assert!(!shadow(&rising, 0, 0).ocean_upwind);
    }

    #[test]
    fn maritime_coast_adds_humidity() {
        // Two grids differ only in an ocean band upwind of cell x = 0. The shadow is
        // zero in both, so the only difference is the coast term.
        let params = wide_params();
        let climate = Climate::new();
        let table = table_2x2();
        let wet = climate.solve(
            &cells(0, 0, |cell_x, _| {
                if (-8..=-4).contains(&cell_x) {
                    SEA_HEIGHT - 4.0
                } else {
                    SEA_HEIGHT + 2.0
                }
            }),
            &table,
            &params,
            17,
        );
        let dry = climate.solve(&cells(0, 0, |_, _| SEA_HEIGHT + 2.0), &table, &params, 17);

        let mut compared = 0;
        for cell_z in MIN_CELL..=MAX_CELL {
            let coastal = wet.humidity(0, cell_z);
            let inland = dry.humidity(0, cell_z);
            assert!(coastal >= inland, "coast was drier at cell_z {cell_z}");
            if inland < 1.0 - MARITIME {
                compared += 1;
                assert!(
                    (coastal - inland - MARITIME).abs() < 1e-5,
                    "coast added {} at cell_z {cell_z}",
                    coastal - inland
                );
            }
        }
        assert!(
            compared >= 8,
            "only {compared} unsaturated columns compared"
        );
    }

    /// `E2.4`: the humidity order is pinned. With no shadow and no coast the
    /// stored value must be exactly `unit(noise)`, bit for bit.
    #[test]
    fn humidity_is_unit_noise_when_shadow_and_coast_are_zero() {
        let params = wide_params();
        let climate = Climate::new();
        let table = table_2x2();
        let seed = 0x5EED;
        let grid = cells(0, 0, |_, _| SEA_HEIGHT + 2.0);
        let tile = climate.solve(&grid, &table, &params, seed);

        let humid_seed = stream_seed(seed, domain(HUMIDITY_DOMAIN));
        for cell_z in MIN_CELL..=MAX_CELL {
            for cell_x in MIN_CELL..=MAX_CELL {
                let march = shadow(&grid, cell_x, cell_z);
                assert_eq!(march.depth, 0.0, "the fixture must have no shadow");
                assert!(!march.ocean_upwind, "the fixture must have no coast");

                let noise = unit(fbm_2d(
                    humid_seed,
                    sample_position(cell_x),
                    sample_position(cell_z),
                    HUMIDITY_NOISE_FREQ,
                    HUMIDITY_NOISE_OCTAVES,
                    NOISE_LACUNARITY,
                    NOISE_GAIN,
                ));
                assert_eq!(
                    tile.humidity(cell_x, cell_z).to_bits(),
                    noise.to_bits(),
                    "humidity at {cell_x},{cell_z} is not the raw unit noise"
                );
            }
        }
        assert_eq!(tile.clamp_fires(), 0);
    }

    /// `E2.4`: multiplying by the shadow before adding maritime is observable.
    /// The swapped expression must differ where both terms are live, and the
    /// stored value must be the pinned one.
    #[test]
    fn humidity_order_cannot_be_swapped() {
        let params = own_shadow_params();
        let climate = Climate::new();
        let table = table_2x2();
        let seed = 0x0DDE;

        // A ridge 12 cells upwind gives a partial shadow; an ocean cell further
        // upwind supplies the coast term, so both are live at cell (0, 0).
        let grid = cells(0, 0, |cell_x, _| {
            if cell_x == -30 {
                SEA_HEIGHT - 20.0
            } else if cell_x == -12 {
                SEA_HEIGHT + 40.0
            } else {
                SEA_HEIGHT + 2.0
            }
        });
        let march = shadow(&grid, 0, 0);
        assert!(march.depth > 0.0 && march.depth < SHADOW_NORM, "{march:?}");
        assert!(
            march.ocean_upwind,
            "the fixture must have a live coast term"
        );

        let tile = climate.solve(&grid, &table, &params, seed);
        let humid_seed = stream_seed(seed, domain(HUMIDITY_DOMAIN));
        let noise = unit(fbm_2d(
            humid_seed,
            sample_position(0),
            sample_position(0),
            HUMIDITY_NOISE_FREQ,
            HUMIDITY_NOISE_OCTAVES,
            NOISE_LACUNARITY,
            NOISE_GAIN,
        ));
        let drying = params.shadow.sample(march.depth / SHADOW_NORM);
        let mut fires = 0_u64;
        let pinned = clamp_unit(
            noise * (1.0 - SHADOW_STRENGTH * drying) + MARITIME,
            &mut fires,
        );
        let mut swapped_fires = 0_u64;
        let swapped = clamp_unit(
            (noise + MARITIME) * (1.0 - SHADOW_STRENGTH * drying),
            &mut swapped_fires,
        );

        assert_ne!(
            pinned.to_bits(),
            swapped.to_bits(),
            "the two orders must be distinguishable here"
        );
        assert_eq!(
            tile.humidity(0, 0).to_bits(),
            pinned.to_bits(),
            "the implementation did not use multiply-then-add"
        );
        assert_ne!(
            tile.humidity(0, 0).to_bits(),
            swapped.to_bits(),
            "the implementation used the swapped order"
        );
    }

    /// The wide latitude span of [`wide_params`] with the non-saturating v1
    /// shadow curve, so the order test exercises a partial shadow.
    fn own_shadow_params() -> ClimateParams {
        ClimateParams {
            shadow: Curve::new(&[(0.0, 0.0), (0.35, 0.18), (0.75, 0.60), (1.0, 1.0)])
                .expect("the v1 shadow knots are valid"),
            ..wide_params()
        }
    }

    #[test]
    fn biome_pick_is_nearest_in_climate_space() {
        let table = table_2x2();
        for (temperature, humidity, expected) in [
            (0.2_f32, 0.2_f32, 1_u8),
            (0.2, 0.8, 2),
            (0.8, 0.2, 3),
            (0.8, 0.8, 4),
        ] {
            let (best, second, _) = table.nearest_two(temperature, humidity, 0.05);
            assert_eq!(best, expected, "at ({temperature}, {humidity})");
            assert_ne!(second, best);
        }

        // Exactly equidistant centres tie, and the lower id wins.
        let (best, second, _) = table.nearest_two(0.5, 0.2, 0.05);
        assert_eq!(best, 1);
        assert_eq!(second, 3);
    }

    #[test]
    fn margin_quantises_to_255_deep_inside() {
        // One biome: `E3.5` pins `second = best` and `q = 255` regardless of climate.
        let single = BiomeTable::new(vec![Biome {
            id: 7,
            temp_c: 0.5,
            humid_c: 0.5,
            weight: 1.0,
            priority: 0,
        }])
        .unwrap();
        let params = wide_params();
        let climate = Climate::new();
        let tile = climate.solve(&cells(0, 0, |_, _| SEA_HEIGHT + 2.0), &single, &params, 5);
        let (cell_x, cell_z) = (3, -1);
        assert_eq!(tile.margin(cell_x, cell_z), 255);
        assert_eq!(tile.best(cell_x, cell_z), 7);
        assert_eq!(tile.second(cell_x, cell_z), 7);

        // `w = 1` deep inside, so every coordinate picks `best`. There is no `y`
        // argument: a column has one pick by construction.
        let tag = domain(DITHER_DOMAIN);
        for block_z in -8..=8 {
            for block_x in -8..=8 {
                let first = tile.biome_pick(cell_x, cell_z, block_x * 64, block_z * 64, 5, tag);
                let again = tile.biome_pick(cell_x, cell_z, block_x * 64, block_z * 64, 5, tag);
                assert_eq!(first, 7);
                assert_eq!(again, first);
            }
        }
        assert_eq!(tile.clamp_fires(), 0);
    }

    #[test]
    fn dither_share_matches_the_margin() {
        let params = wide_params();
        let climate = Climate::new();
        let tile = climate.solve(
            &cells(0, 0, |_, _| SEA_HEIGHT + 2.0),
            &table_2x2(),
            &params,
            9,
        );

        // Find a cell that is neither on a seam nor deep inside, so both outcomes are
        // reachable.
        let mut target = None;
        for cell_z in MIN_CELL..=MAX_CELL {
            for cell_x in MIN_CELL..=MAX_CELL {
                let q = tile.margin(cell_x, cell_z);
                if (40..=200).contains(&q) {
                    target = Some((cell_x, cell_z, q));
                    break;
                }
            }
            if target.is_some() {
                break;
            }
        }
        let (cell_x, cell_z, q) = target.expect("a mid-margin cell exists in this fixture");
        let weight = 0.5 + 0.5 * smoothstep01(f32::from(q) / 255.0);
        assert!(weight > 0.5 && weight < 1.0);

        let tag = domain(DITHER_DOMAIN);
        let mut near = 0_u32;
        let mut total = 0_u32;
        for block_z in -32..32 {
            for block_x in -32..32 {
                let pick = tile.biome_pick(cell_x, cell_z, block_x, block_z, 17, tag);
                assert!(pick == tile.best(cell_x, cell_z) || pick == tile.second(cell_x, cell_z));
                if pick == tile.best(cell_x, cell_z) {
                    near += 1;
                }
                total += 1;
            }
        }
        let share = f64::from(near) / f64::from(total);
        assert!(
            (share - f64::from(weight)).abs() <= 0.02,
            "share {share} vs w {weight} at q {q}"
        );
    }

    #[test]
    fn border_cells_match_the_neighbour() {
        let params = wide_params();
        let climate = Climate::new();
        let table = table_2x2();
        let height = |cell_x: i32, cell_z: i32| {
            SEA_HEIGHT + 40.0 + cell_x as f32 * 0.25 - cell_z as f32 * 0.5
        };

        let here = climate.solve_at(0, 0, &cells(0, 0, height), &table, &params, 21);
        let neighbour = climate.solve_at(32, 0, &cells(32, 0, height), &table, &params, 21);

        for cell_z in MIN_CELL..=MAX_CELL {
            for local in [31, 32] {
                let away = local - 32;
                assert!((MIN_CELL..=MAX_CELL).contains(&away));
                assert_eq!(
                    here.temp(local, cell_z).to_bits(),
                    neighbour.temp(away, cell_z).to_bits()
                );
                assert_eq!(
                    here.humidity(local, cell_z).to_bits(),
                    neighbour.humidity(away, cell_z).to_bits()
                );
                assert_eq!(here.best(local, cell_z), neighbour.best(away, cell_z));
                assert_eq!(here.second(local, cell_z), neighbour.second(away, cell_z));
                assert_eq!(here.margin(local, cell_z), neighbour.margin(away, cell_z));
            }
        }
        assert_eq!(here.clamp_fires(), 0);
        assert_eq!(neighbour.clamp_fires(), 0);
    }

    #[test]
    fn bad_tables_are_rejected() {
        let good = Biome {
            id: 1,
            temp_c: 0.5,
            humid_c: 0.5,
            weight: 1.0,
            priority: 0,
        };
        let rejected = |biomes: Vec<Biome>| {
            matches!(
                BiomeTable::new(biomes),
                Err(TerrainError {
                    kind: ErrorKind::DescriptionInvalid { key: "BIOMES", .. },
                    ..
                })
            )
        };

        assert!(rejected(vec![good, good]), "duplicate id and centre");
        assert!(
            rejected(vec![good, Biome { id: 2, ..good },]),
            "duplicate centre"
        );
        assert!(
            rejected(vec![Biome {
                temp_c: 1.5,
                ..good
            }]),
            "temperature centre out of range"
        );
        assert!(
            rejected(vec![Biome {
                humid_c: -0.1,
                ..good
            }]),
            "humidity centre out of range"
        );
        assert!(
            rejected(vec![Biome {
                weight: 0.0,
                ..good
            }]),
            "zero weight"
        );

        // 256 entries: a 16x16 grid of distinct centres.
        let many: Vec<Biome> = (0..16)
            .flat_map(|temp_index| {
                (0..16).map(move |humid_index| Biome {
                    id: (temp_index * 16 + humid_index) as u8,
                    temp_c: temp_index as f32 / 15.0,
                    humid_c: humid_index as f32 / 15.0,
                    weight: 1.0,
                    priority: 0,
                })
            })
            .collect();
        assert_eq!(many.len(), 256);
        assert!(rejected(many), "256 biomes");

        assert!(rejected(Vec::new()), "empty table");
        assert!(BiomeTable::new(vec![good]).is_ok());
    }
}
