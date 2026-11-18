//! Time of day, sun/moon colours, and the fog factor.
//!
//! A full cycle is 600 seconds. Colours are linear RGB and interpolate between
//! four keyframes: dawn, day, dusk, and night.

use std::f32::consts::TAU;

/// Seconds for a full day/night cycle.
pub(crate) const CYCLE_SECONDS: f32 = 600.0;

/// Time added by the debug hour skip.
pub(crate) const HOUR_STEP: f32 = 1.0 / 24.0;

/// Fog starts at this fraction of the camera far plane.
#[cfg(test)]
const FOG_START: f32 = 0.7;

/// One keyframe: time, sky colour, ambient colour, sun colour.
type Keyframe = (f32, [f32; 3], [f32; 3], [f32; 3]);

const KEYFRAMES: [Keyframe; 4] = [
    (
        0.00,
        [0.55, 0.35, 0.30],
        [0.32, 0.28, 0.30],
        [1.00, 0.65, 0.45],
    ),
    (
        0.25,
        [0.45, 0.65, 0.90],
        [0.65, 0.70, 0.75],
        [1.00, 0.95, 0.85],
    ),
    (
        0.50,
        [0.50, 0.30, 0.25],
        [0.28, 0.25, 0.27],
        [1.00, 0.55, 0.35],
    ),
    (
        0.75,
        [0.02, 0.03, 0.08],
        [0.06, 0.08, 0.14],
        [0.50, 0.60, 0.80],
    ),
];

/// Current time of day and the colours derived from it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Sky {
    time_of_day: f32,
}

/// Sampled colours and sun direction for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SkySample {
    pub(crate) sky: [f32; 3],
    pub(crate) ambient: [f32; 3],
    pub(crate) sun: [f32; 3],
    pub(crate) sun_direction: [f32; 3],
}

impl Default for Sky {
    fn default() -> Self {
        Self::new()
    }
}

impl Sky {
    /// Starts at `0.25`, which is noon.
    pub(crate) fn new() -> Self {
        Self { time_of_day: 0.25 }
    }

    pub(crate) fn time_of_day(self) -> f32 {
        self.time_of_day
    }

    /// Advances the cycle by real time and wraps.
    pub(crate) fn advance(&mut self, delta_seconds: f32) {
        self.time_of_day = (self.time_of_day + delta_seconds / CYCLE_SECONDS).rem_euclid(1.0);
    }

    /// Skips one hour, used by the `T` debug key.
    pub(crate) fn skip_hour(&mut self) {
        self.time_of_day = (self.time_of_day + HOUR_STEP).rem_euclid(1.0);
    }

    /// Interpolates the keyframes at the current time.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss
    )]
    pub(crate) fn sample(self) -> SkySample {
        let scaled = self.time_of_day.rem_euclid(1.0) * KEYFRAMES.len() as f32;
        let index = (scaled.floor() as usize) % KEYFRAMES.len();
        let next = (index + 1) % KEYFRAMES.len();
        let blend = scaled - scaled.floor();
        let (_, sky_a, ambient_a, sun_a) = KEYFRAMES[index];
        let (_, sky_b, ambient_b, sun_b) = KEYFRAMES[next];
        let angle = TAU * self.time_of_day;

        SkySample {
            sky: lerp(sky_a, sky_b, blend),
            ambient: lerp(ambient_a, ambient_b, blend),
            sun: lerp(sun_a, sun_b, blend),
            sun_direction: [angle.cos(), angle.sin(), 0.0],
        }
    }
}

/// Linear fog factor between 70% and 100% of the fog distance.
///
/// This mirrors the shader formula so the range is covered by CPU tests.
#[cfg(test)]
pub(crate) fn fog_factor(distance: f32, fog_end: f32) -> f32 {
    if fog_end <= 0.0 {
        return 1.0;
    }
    let start = FOG_START * fog_end;
    ((distance - start) / ((1.0 - FOG_START) * fog_end)).clamp(0.0, 1.0)
}

fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cycle_starts_at_noon() {
        let sky = Sky::new();

        assert_eq!(sky.time_of_day(), 0.25);
        let [x, y, z] = sky.sample().sun_direction;
        assert!(x.abs() < 1e-6);
        assert!((y - 1.0).abs() < 1e-6);
        assert!(z.abs() < 1e-6);
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn keyframes_have_the_spec_colours() {
        let expected = [
            ([0.55, 0.35, 0.30], [0.32, 0.28, 0.30], [1.00, 0.65, 0.45]),
            ([0.45, 0.65, 0.90], [0.65, 0.70, 0.75], [1.00, 0.95, 0.85]),
            ([0.50, 0.30, 0.25], [0.28, 0.25, 0.27], [1.00, 0.55, 0.35]),
            ([0.02, 0.03, 0.08], [0.06, 0.08, 0.14], [0.50, 0.60, 0.80]),
        ];

        for (index, (sky_colour, ambient, sun)) in expected.into_iter().enumerate() {
            let mut sky = Sky::new();
            sky.time_of_day = index as f32 * 0.25;
            let sample = sky.sample();
            assert_eq!(sample.sky, sky_colour, "sky {index}");
            assert_eq!(sample.ambient, ambient, "ambient {index}");
            assert_eq!(sample.sun, sun, "sun {index}");
        }
    }

    fn assert_close(actual: [f32; 3], expected: [f32; 3]) {
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
        }
    }

    #[test]
    fn colours_interpolate_between_keyframes() {
        let mut sky = Sky::new();
        sky.time_of_day = 0.125;
        let sample = sky.sample();

        assert_close(sample.sky, [0.5, 0.5, 0.6]);
        assert_close(sample.ambient, [0.485, 0.49, 0.525]);
    }

    #[test]
    fn interpolation_wraps_from_night_to_dawn() {
        let mut sky = Sky::new();
        sky.time_of_day = 0.875;
        let sample = sky.sample();

        assert_close(sample.sky, [0.285, 0.19, 0.19]);
    }

    #[test]
    fn sun_direction_tracks_the_cycle() {
        let mut sky = Sky::new();
        sky.time_of_day = 0.25;
        assert!((sky.sample().sun_direction[1] - 1.0).abs() < 1e-6);

        sky.time_of_day = 0.75;
        assert!((sky.sample().sun_direction[1] + 1.0).abs() < 1e-6);

        sky.time_of_day = 0.0;
        let [x, y, z] = sky.sample().sun_direction;
        assert!((x - 1.0).abs() < 1e-6);
        assert!(y.abs() < 1e-6);
        assert!(z.abs() < 1e-6);
    }

    #[test]
    fn time_advances_by_real_seconds_and_wraps() {
        let mut sky = Sky::new();

        sky.advance(CYCLE_SECONDS / 4.0);
        assert!((sky.time_of_day() - 0.5).abs() < 1e-6);

        sky.advance(CYCLE_SECONDS * 2.0);
        assert!((sky.time_of_day() - 0.5).abs() < 1e-6);

        sky.advance(-CYCLE_SECONDS * 0.25);
        assert!((sky.time_of_day() - 0.25).abs() < 1e-6);
    }

    #[test]
    fn the_hour_skip_wraps() {
        let mut sky = Sky::new();

        sky.skip_hour();
        assert!((sky.time_of_day() - (0.25 + HOUR_STEP)).abs() < 1e-6);

        for _ in 0..24 {
            sky.skip_hour();
        }
        assert!((sky.time_of_day() - (0.25 + HOUR_STEP)).abs() < 1e-4);
    }

    #[test]
    fn fog_runs_from_seventy_to_one_hundred_percent() {
        assert_eq!(fog_factor(0.0, 100.0), 0.0);
        assert_eq!(fog_factor(69.0, 100.0), 0.0);
        assert!((fog_factor(85.0, 100.0) - 0.5).abs() < 1e-6);
        assert!((fog_factor(100.0, 100.0) - 1.0).abs() < 1e-6);
        assert_eq!(fog_factor(500.0, 100.0), 1.0);
    }

    #[test]
    fn fog_uses_the_render_distance() {
        let fog_end = 12.0 * 32.0;

        assert_eq!(fog_factor(0.7 * fog_end - 1.0, fog_end), 0.0);
        assert!((fog_factor(0.85 * fog_end, fog_end) - 0.5).abs() < 1e-4);
        assert!((fog_factor(fog_end, fog_end) - 1.0).abs() < 1e-6);
        assert_eq!(fog_factor(500.0, fog_end), 1.0);
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn the_sun_axis_is_horizontal() {
        let mut sky = Sky::new();
        for step in 0..12 {
            sky.time_of_day = step as f32 / 12.0;
            assert_eq!(sky.sample().sun_direction[2], 0.0);
        }
    }
}
