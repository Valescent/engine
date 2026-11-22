//! Climate parameters and the latitude and altitude curve terms (`E1.1`, `E1.2`).

use crate::curve::Curve;
use crate::error::{ErrorKind, Stage, TerrainError};

use super::field::SEA_HEIGHT;

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
pub(crate) fn blend_ramp_blocks(blend_blocks: u32) -> f32 {
    blend_blocks as f32 / (2.0 * S95)
}

/// Per-world climate parameters (`E1.1`, `E1.2`, `E3.3`).
///
/// Every field is a constructor value rather than a constant because the description
/// owns them (`A1.3`). The two curves are compiled once and shared by every tile.
#[derive(Clone, Debug)]
pub(crate) struct ClimateParams {
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
    pub(crate) fn v1_default() -> Self {
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
    #[allow(dead_code)]
    pub(crate) fn validate(&self) -> Result<(), TerrainError> {
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

/// Temperature's latitude term alone: `lat(|z - equator| / half_span)` (`E1.1`).
///
/// Exposed for debug views and tests; the solver composes it with noise and altitude.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub(crate) fn latitude(params: &ClimateParams, block_z: i32) -> f32 {
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
pub(crate) fn altitude(params: &ClimateParams, macro_height: f32) -> f32 {
    (macro_height - SEA_HEIGHT).max(0.0) * params.lapse
}

/// Builds a typed description error with the fixed stage and kind.
pub(super) fn invalid(key: &'static str, rule: &'static str) -> TerrainError {
    TerrainError::new(
        Stage::Description,
        ErrorKind::DescriptionInvalid { key, rule },
    )
}
