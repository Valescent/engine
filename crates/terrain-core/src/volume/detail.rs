//! The depth-limited detail band: band weight, block sample, and the pass.

use crate::noise::simplex2_3d;
use crate::section::BlockId;

use super::{COLUMN_HEIGHT, ColumnInput, Volume, VolumeStats, WORLD_LAST_Y, ramp};

/// Noise taps a single detail sample may spend (`F3.2`).
///
/// One 3D tap today; the budget is the ceiling the band is allowed to grow into.
pub const DETAIL_TAP_BUDGET: u8 = 4;

/// Density a floating cell must clear to become solid (`F3.2`).
pub const FLOAT_MIN: f32 = 0.15;

/// Frequency of the detail field, in cycles per block (`F3.2`).
const DETAIL_FREQ: f32 = 1.0 / 40.0;

/// Blocks below sea at which the detail band is fully faded out (`F3.4`).
const DETAIL_COAST_OFFSET: i32 = 2;

/// Blocks over which the detail coast mask fades in.
const DETAIL_COAST_FADE: f32 = 24.0;

/// Divisor of the detail height term (`F3.2`).
pub(super) const DETAIL_SLOPE_SCALE: f32 = 8.0;

const _: () = assert!(DETAIL_COAST_OFFSET > 0);

/// Detail curve: x is column roughness, y is the mask (`F3.2`).
pub(super) const DETAIL_KNOTS: &[(f32, f32)] = &[
    (0.0, 0.0),
    (0.30, 0.15),
    (0.60, 0.55),
    (0.85, 0.90),
    (1.0, 1.0),
];

/// Slope curve: x is the column's integer slope drop, y is the mask (`F3.2`).
pub(super) const SLOPE_KNOTS: &[(f32, f32)] =
    &[(0.0, 0.0), (1.0, 0.30), (2.0, 0.65), (4.0, 1.0), (8.0, 1.0)];

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

impl Volume {
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

    /// `F1.5` step 3: edit solidity inside the detail band.
    pub(super) fn detail_pass(
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

    /// `F3.4`: detail has no business over shallow water, a bed, or a bank.
    fn detail_allowed(&self, input: &ColumnInput) -> bool {
        input.top > self.params.sea + DETAIL_COAST_OFFSET
            && !input.is_river
            && !input.is_lake
            && !input.near_bank
    }
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
