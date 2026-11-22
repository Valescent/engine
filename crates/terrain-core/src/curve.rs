//! W2 - Fritsch-Carlson monotone cubic curves and the `linear` variant.
//!
//! `B2.2`. A curve is compiled once from control points with strictly increasing `x`
//! into one cubic per segment, stored in Horner form. Control points and outputs are
//! `[0, 1]` values, `y` is rejected outside that range, and the input is clamped to
//! the end points. The cubic is monotone by construction, so an output can never leave
//! the endpoint range. Flat segments get zero tangents.
//!
//! `Curve::linear` compiles straight segments instead of cubics. It is the variant
//! for terraces and hard cliffs, where a rounded corner would erase the feature.
//!
//! No `mul_add` and no transcendental functions are used; every operation is `+ - * /`
//! in a fixed order.

use crate::error::{ErrorKind, Stage, TerrainError};

/// One compiled segment of a curve.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Segment {
    /// Left end point of the segment on the `x` axis.
    x: f32,
    /// Value at `x`.
    a0: f32,
    /// First coefficient of the local Horner polynomial.
    a1: f32,
    /// Second coefficient of the local Horner polynomial.
    a2: f32,
    /// Third coefficient of the local Horner polynomial.
    a3: f32,
}

/// A compiled monotone cubic curve over `[0, 1]`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Curve {
    /// Control points, strictly increasing in `x`.
    points: Vec<(f32, f32)>,
    /// One compiled polynomial per interval between control points.
    segments: Vec<Segment>,
    /// Whether the curve was compiled as straight segments.
    linear: bool,
}

impl Curve {
    /// Builds a Fritsch-Carlson monotone cubic from control points.
    ///
    /// # Errors
    ///
    /// Rejects fewer than two points, non-finite values, a `y` outside `[0, 1]`, and
    /// an `x` sequence that is not strictly increasing.
    pub(crate) fn new(points: &[(f32, f32)]) -> Result<Self, TerrainError> {
        Self::compile(points, false)
    }

    /// Builds a piecewise-linear curve for terraces and hard cliffs.
    ///
    /// Every segment is a straight line, so its second and third coefficients are zero
    /// and a flat segment has zero slope. Corners at the knots are kept hard instead of
    /// being rounded by a cubic.
    ///
    /// # Errors
    ///
    /// Same validation as [`Curve::new`].
    #[allow(dead_code)]
    pub(crate) fn linear(points: &[(f32, f32)]) -> Result<Self, TerrainError> {
        Self::compile(points, true)
    }

    fn compile(points: &[(f32, f32)], linear: bool) -> Result<Self, TerrainError> {
        validate(points)?;
        let segments = if linear {
            linear_segments(points)
        } else {
            cubic_segments(points)
        };
        Ok(Self {
            points: points.to_vec(),
            segments,
            linear,
        })
    }

    /// Samples the curve at `x`, clamped to the end points.
    #[must_use]
    pub(crate) fn sample(&self, x: f32) -> f32 {
        let Some(first) = self.points.first() else {
            return 0.0;
        };
        let Some(last) = self.points.last() else {
            return 0.0;
        };
        if x <= first.0 {
            return first.1;
        }
        if x >= last.0 {
            return last.1;
        }
        let segment = &self.segments[self.segment_index(x)];
        let t = x - segment.x;
        segment.a0 + t * (segment.a1 + t * (segment.a2 + t * segment.a3))
    }

    /// Control points in `x` order.
    #[must_use]
    pub(crate) fn points(&self) -> &[(f32, f32)] {
        &self.points
    }

    /// Whether the curve is the piecewise-linear variant.
    #[allow(dead_code)]
    #[must_use]
    pub(crate) fn is_linear(&self) -> bool {
        self.linear
    }

    fn segment_index(&self, x: f32) -> usize {
        let last = self.segments.len() - 1;
        if self.points.len() <= 8 {
            let mut index = 0;
            while index < last && x >= self.points[index + 1].0 {
                index += 1;
            }
            index
        } else {
            self.points
                .partition_point(|point| point.0 <= x)
                .saturating_sub(1)
                .min(last)
        }
    }
}

fn invalid(rule: &'static str) -> TerrainError {
    TerrainError::new(
        Stage::Description,
        ErrorKind::DescriptionInvalid { key: "CURVE", rule },
    )
}

fn validate(points: &[(f32, f32)]) -> Result<(), TerrainError> {
    if points.len() < 2 {
        return Err(invalid("at least two points"));
    }
    for (x, y) in points {
        if !x.is_finite() || !y.is_finite() {
            return Err(invalid("finite"));
        }
        if !(0.0..=1.0).contains(y) {
            return Err(invalid("y range"));
        }
    }
    for pair in points.windows(2) {
        if pair[1].0 <= pair[0].0 {
            return Err(invalid("x increasing"));
        }
    }
    Ok(())
}

fn linear_segments(points: &[(f32, f32)]) -> Vec<Segment> {
    points
        .windows(2)
        .map(|pair| {
            let (x0, y0) = pair[0];
            let (x1, y1) = pair[1];
            Segment {
                x: x0,
                a0: y0,
                a1: (y1 - y0) / (x1 - x0),
                a2: 0.0,
                a3: 0.0,
            }
        })
        .collect()
}

fn cubic_segments(points: &[(f32, f32)]) -> Vec<Segment> {
    let count = points.len();
    let mut slopes = Vec::with_capacity(count - 1);
    for pair in points.windows(2) {
        slopes.push((pair[1].1 - pair[0].1) / (pair[1].0 - pair[0].0));
    }

    let mut tangents = vec![0.0_f32; count];
    tangents[0] = slopes[0];
    tangents[count - 1] = slopes[count - 2];
    for index in 1..count - 1 {
        tangents[index] = f32::midpoint(slopes[index - 1], slopes[index]);
    }
    for index in 0..count - 1 {
        limit_tangents(&mut tangents, &slopes, index);
    }

    points
        .windows(2)
        .enumerate()
        .map(|(index, pair)| {
            let (x0, y0) = pair[0];
            let width = pair[1].0 - x0;
            let delta = slopes[index];
            let left = tangents[index];
            let right = tangents[index + 1];
            Segment {
                x: x0,
                a0: y0,
                a1: left,
                a2: (3.0 * delta - 2.0 * left - right) / width,
                a3: (left + right - 2.0 * delta) / (width * width),
            }
        })
        .collect()
}

/// Applies the Fritsch-Carlson monotonicity limiter to one segment.
fn limit_tangents(tangents: &mut [f32], slopes: &[f32], index: usize) {
    let delta = slopes[index];
    if delta == 0.0 {
        tangents[index] = 0.0;
        tangents[index + 1] = 0.0;
        return;
    }
    let alpha = tangents[index] / delta;
    let beta = tangents[index + 1] / delta;
    let magnitude = alpha * alpha + beta * beta;
    if magnitude > 9.0 {
        let scale = 3.0 / magnitude.sqrt();
        tangents[index] = scale * alpha * delta;
        tangents[index + 1] = scale * beta * delta;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::{domain, hash01, hash2};

    fn knot(seed: u64, index: i32, axis: i32) -> f32 {
        hash01(hash2(seed, domain("w2-curve"), index, axis))
    }

    #[test]
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    fn curve_is_monotone_and_bounded() {
        for case in 0..128_u64 {
            let count = 2 + (case % 11) as usize;
            let mut points = Vec::with_capacity(count);
            let mut x = 0.0_f32;
            let mut y = 0.0_f32;
            for index in 0..count {
                let index = i32::try_from(index).unwrap_or(i32::MAX);
                x += 0.05 + knot(case, index, 0) * 0.5;
                let step = knot(case, index, 1) * 0.9 / count as f32;
                y = (y + step).min(1.0);
                points.push((x, y));
            }
            assert_curve_properties(&points, false);
            assert_curve_properties(&points, true);
        }
    }

    #[allow(clippy::cast_precision_loss)]
    fn assert_curve_properties(points: &[(f32, f32)], linear: bool) {
        let curve = if linear {
            Curve::linear(points).unwrap()
        } else {
            Curve::new(points).unwrap()
        };
        let first = points[0].0;
        let last = points[points.len() - 1].0;
        let low = points[0].1;
        let high = points[points.len() - 1].1;
        let mut previous = curve.sample(first - 100.0);
        assert_eq!(previous, low, "clamped below the first knot");

        for step in 0..=400 {
            let x = first - 10.0 + (last - first + 20.0) * step as f32 / 400.0;
            let value = curve.sample(x);
            assert!(
                value >= low - 1e-5 && value <= high + 1e-5,
                "{value} outside [{low}, {high}] at {x}"
            );
            assert!(value >= previous - 1e-5, "not monotone at {x}");
            previous = value;
        }
        assert_eq!(
            curve.sample(last + 100.0),
            high,
            "clamped above the last knot"
        );
    }

    #[test]
    fn curve_rejects_invalid_knots() {
        assert!(Curve::new(&[(0.0, 0.0)]).is_err());
        assert!(Curve::new(&[(0.0, 0.0), (0.0, 1.0)]).is_err());
        assert!(Curve::new(&[(0.0, 0.0), (1.0, 0.5), (0.5, 1.0)]).is_err());
        assert!(Curve::new(&[(0.0, f32::NAN), (1.0, 1.0)]).is_err());
        assert!(Curve::new(&[(0.0, 0.0), (1.0, f32::INFINITY)]).is_err());
        assert!(Curve::new(&[(0.0, 0.0), (1.0, 1.5)]).is_err());
        assert!(Curve::new(&[(0.0, -0.1), (1.0, 1.0)]).is_err());
        assert!(Curve::linear(&[(0.0, 0.0)]).is_err());
        for error in [
            Curve::new(&[(0.0, 0.0)]),
            Curve::new(&[(0.0, 0.0), (0.0, 1.0)]),
        ] {
            assert!(matches!(
                error,
                Err(TerrainError {
                    kind: ErrorKind::DescriptionInvalid { key: "CURVE", .. },
                    ..
                })
            ));
        }
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn flat_segments_stay_flat() {
        let curve = Curve::new(&[(0.0, 0.25), (0.5, 0.25), (1.0, 0.75)]).unwrap();
        for step in 0..=50 {
            let x = step as f32 / 100.0;
            assert_eq!(curve.sample(x), 0.25, "flat segment at {x}");
        }
        assert_eq!(curve.sample(1.0), 0.75);
    }

    #[test]
    fn linear_variant_is_exact_between_knots() {
        let curve = Curve::linear(&[(0.0, 0.0), (0.5, 0.25), (1.0, 1.0)]).unwrap();
        assert!(curve.is_linear());
        assert_eq!(curve.sample(0.25), 0.125);
        assert_eq!(curve.sample(0.5), 0.25);
        assert_eq!(curve.sample(0.75), 0.625);
        assert_eq!(curve.sample(-10.0), 0.0);
        assert_eq!(curve.sample(10.0), 1.0);
        assert_eq!(curve.points().len(), 3);
    }

    #[test]
    fn cubic_is_smooth_at_the_knots() {
        let curve = Curve::new(&[(0.0, 0.0), (0.4, 0.1), (0.7, 0.8), (1.0, 0.9)]).unwrap();
        let epsilon = 1e-4;
        for point in curve.points().iter().skip(1) {
            let left = curve.sample(point.0 - epsilon);
            let right = curve.sample(point.0 + epsilon);
            let midpoint = f32::midpoint(left, right);
            assert!(
                (midpoint - point.1).abs() < 1e-3,
                "knot {} value {} vs {}",
                point.0,
                midpoint,
                point.1
            );
        }
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn many_knots_use_binary_search_and_stay_monotone() {
        let mut points = Vec::new();
        for index in 0..40_i32 {
            let y = index as f32 / 39.0;
            points.push((index as f32 / 39.0, y));
        }
        let curve = Curve::new(&points).unwrap();
        let mut previous = -1.0;
        for step in 0..=2000 {
            let x = step as f32 / 2000.0;
            let value = curve.sample(x);
            assert!(value >= previous - 1e-6);
            assert!((0.0..=1.0).contains(&value));
            previous = value;
        }
    }
}
