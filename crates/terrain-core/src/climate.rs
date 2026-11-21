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
//! caller-supplied monotone [`crate::curve::Curve`] that is warm at 0 and cold at 1,
//! `noise` is a three-octave fBm at 1/512 cycles per block scaled by [`TEMP_NOISE_AMP`],
//! and `alt = max(h_macro - SEA, 0) * lapse` (`E1.1`, `E1.2`). Latitude and the noise are
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
//!
//! # Submodules
//!
//! - `params`: [`ClimateParams`], the blend ramp, and the latitude and altitude terms.
//! - `table`: [`Biome`] centres, nearest-two selection, and the normalised quantiser.
//! - `field`: [`MacroCells`], the rain-shadow march, [`ClimateTile`], and [`Climate`].

mod field;
mod params;
mod table;

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
mod tests;

pub use field::{
    CELL_COUNT, Climate, ClimateTile, DITHER_DOMAIN, HUMIDITY_DOMAIN, MARITIME, MAX_CELL, MIN_CELL,
    MacroCells, PADDED_CELLS, SHADOW_NORM, SHADOW_RANGE_CELLS, SHADOW_STRENGTH, SIDE, SLOPE_K,
    Shadow, TEMP_DOMAIN, TEMP_NOISE_AMP, WIND_DIR, index, padded_index, shadow,
};
pub use params::{ClimateParams, altitude, blend_ramp_blocks, latitude};
pub use table::{Biome, BiomeTable, H_SCALE, T_SCALE, smoothstep01};
