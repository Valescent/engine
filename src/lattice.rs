//! W2 - the 4-block macro lattice: `macro_cell`, `floor_div`, `rem_euclid` helpers.
//!
//! `B4.5`, `C3.3`. Samples sit at `4k + 0.5`, the centre of block `4k`. Block `x` is in
//! cell `k = floor_div(x, 4)` with weight `rem_euclid(x, 4) / 4`, interpolated between
//! cells `k` and `k + 1`. The lattice is global in world coordinates, never chunk
//! anchored, so a tile generates independently of its neighbours.
//!
//! Signed division and remainder are written out by hand. Rust's `/` and `%` truncate
//! toward zero, which puts `-1` in cell `0` with a negative weight; both helpers here
//! floor instead, so `-1` is in cell `-1` with weight `0.75`. Weights are exact quarters,
//! so bilinear interpolation over quarter weights is exact in `f32`.

use crate::description::MACRO_CELL;

/// Returns `value / divisor`, rounded toward negative infinity.
///
/// `divisor` must be positive. The division is performed on unsigned magnitudes, so
/// neither `/` nor `%` ever sees a negative operand.
///
/// # Panics
///
/// Panics when `divisor <= 0`.
#[must_use]
pub fn floor_div(value: i32, divisor: i32) -> i32 {
    assert!(divisor > 0, "floor_div requires a positive divisor");
    let magnitude = i64::from(value).unsigned_abs();
    let divisor = u64::from(divisor.cast_unsigned());
    let quotient = magnitude / divisor;
    let remainder = magnitude % divisor;
    if value < 0 {
        let floored = if remainder == 0 {
            quotient
        } else {
            quotient + 1
        };
        match i32::try_from(floored) {
            Ok(quotient) => -quotient,
            // Only `i32::MIN / 1` can land exactly on `2^31`, whose floor is
            // `i32::MIN` itself.
            Err(_) => i32::MIN,
        }
    } else {
        i32::try_from(quotient).unwrap_or(i32::MAX)
    }
}

/// Returns the non-negative remainder of `value` divided by `divisor`.
///
/// `divisor` must be positive. The remainder is computed on the unsigned magnitude and
/// mirrored into `0..divisor`, so it never inherits the sign of `value`.
///
/// # Panics
///
/// Panics when `divisor <= 0`.
#[must_use]
pub fn rem_euclid(value: i32, divisor: i32) -> i32 {
    assert!(divisor > 0, "rem_euclid requires a positive divisor");
    let magnitude = i64::from(value).unsigned_abs();
    let divisor = u64::from(divisor.cast_unsigned());
    let remainder = magnitude % divisor;
    let remainder = if value < 0 && remainder != 0 {
        divisor - remainder
    } else {
        remainder
    };
    i32::try_from(remainder).unwrap_or(i32::MAX)
}

/// Returns the macro cell index and the exact quarter weight of one block coordinate.
///
/// The result is `(floor_div(block, MACRO_CELL), rem_euclid(block, MACRO_CELL) / 4)`.
#[allow(clippy::cast_precision_loss)]
#[must_use]
pub fn macro_cell(block: i32) -> (i32, f32) {
    let cell = floor_div(block, MACRO_CELL);
    let weight = rem_euclid(block, MACRO_CELL) as f32 / MACRO_CELL as f32;
    (cell, weight)
}

/// Returns the world position of a lattice sample for one block coordinate.
///
/// A sample sits at the centre of its block: `block + 0.5`.
#[allow(clippy::cast_precision_loss)]
#[must_use]
pub fn sample_pos(block: i32) -> f32 {
    block as f32 + 0.5
}

/// Bilinearly interpolates four cell corners to one block position.
///
/// `c00` is the corner at `(k, j)`, `c10` at `(k + 1, j)`, `c01` at `(k, j + 1)`,
/// and `c11` at `(k + 1, j + 1)`. `wx` and `wz` are the exact quarter weights in
/// `0..=1`. The operation order is fixed: interpolate along `x`, then along `z`. No
/// `mul_add` is used, so the result is bit-stable for a given build.
#[must_use]
pub fn bilinear(c00: f32, c10: f32, c01: f32, c11: f32, wx: f32, wz: f32) -> f32 {
    let near = c00 + (c10 - c00) * wx;
    let far = c01 + (c11 - c01) * wx;
    near + (far - near) * wz
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macro_cell_handles_negatives() {
        assert_eq!(macro_cell(-1), (-1, 0.75));
        assert_eq!(macro_cell(-4), (-1, 0.0));
        assert_eq!(macro_cell(-5), (-2, 0.75));
        assert_eq!(macro_cell(0), (0, 0.0));
        assert_eq!(macro_cell(3), (0, 0.75));
        assert_eq!(macro_cell(4), (1, 0.0));
    }

    #[test]
    fn floor_div_and_rem_euclid_agree_with_the_standard_library() {
        for divisor in [1, 2, 3, 4, 5, 7, 16, 127, 1024] {
            for value in -100_000..=100_000 {
                assert_eq!(floor_div(value, divisor), value.div_euclid(divisor));
                assert_eq!(rem_euclid(value, divisor), value.rem_euclid(divisor));
            }
            for value in [
                i32::MIN + 1,
                i32::MIN + 2,
                i32::MAX - 1,
                i32::MAX,
                -i32::MAX,
            ] {
                assert_eq!(floor_div(value, divisor), value.div_euclid(divisor));
                assert_eq!(rem_euclid(value, divisor), value.rem_euclid(divisor));
            }
            assert_eq!(floor_div(i32::MIN, divisor), i32::MIN.div_euclid(divisor));
            assert_eq!(rem_euclid(i32::MIN, divisor), i32::MIN.rem_euclid(divisor));
        }
    }

    #[test]
    fn floor_div_and_rem_euclid_reconstruct_the_value() {
        for divisor in [1, 4, 9, 256] {
            for value in (-5000..=5000).step_by(7) {
                let cell = floor_div(value, divisor);
                let weight = rem_euclid(value, divisor);
                assert_eq!(cell * divisor + weight, value);
                assert!((0..divisor).contains(&weight));
            }
        }
    }

    #[test]
    fn bilinear_matches_corners() {
        let (c00, c10, c01, c11) = (0.25_f32, 0.5, 0.75, 1.0);

        assert_eq!(bilinear(c00, c10, c01, c11, 0.0, 0.0), c00);
        assert_eq!(bilinear(c00, c10, c01, c11, 1.0, 0.0), c10);
        assert_eq!(bilinear(c00, c10, c01, c11, 0.0, 1.0), c01);
        assert_eq!(bilinear(c00, c10, c01, c11, 1.0, 1.0), c11);
        // Quarter weights are exact in `f32`, so the centre is the exact mean of
        // quarter-valued corners.
        assert_eq!(bilinear(c00, c10, c01, c11, 0.5, 0.5), 0.625);
    }

    #[test]
    fn bilinear_is_constant_for_constant_corners() {
        for wx in [0.0, 0.25, 0.5, 0.75, 1.0] {
            for wz in [0.0, 0.25, 0.5, 0.75, 1.0] {
                assert_eq!(bilinear(0.5, 0.5, 0.5, 0.5, wx, wz), 0.5);
            }
        }
    }

    #[test]
    fn samples_sit_at_block_centres() {
        assert_eq!(sample_pos(0), 0.5);
        assert_eq!(sample_pos(-1), -0.5);
        assert_eq!(sample_pos(4), 4.5);
        assert_eq!(macro_cell(0).0, 0);
        assert_eq!(sample_pos(rem_euclid(4, MACRO_CELL)), 0.5);
    }
}
