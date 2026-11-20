//! W2 - `OpenSimplex2` sampling, `unit()`, fBm/ridged/billow, measured bounds.
//!
//! `B1.1`, `B1.2`, `B1.7`, `B2.1`. One algorithm ships: `OpenSimplex2`. Variety comes
//! from composition, not from a second sampler.
//!
//! Range contract, in order:
//!
//! 1. [`simplex2_2d`] and [`simplex2_3d`] return a raw sample divided by a measured,
//!    pinned bound, so every octave is inside `[-1, 1]` by construction. A debug
//!    assertion checks that the library sample itself is inside `[-1, 1]`.
//! 2. [`fbm_2d`], [`ridged_2d`] and [`billow_2d`] sum octaves in a fixed order and
//!    divide once by the sum of amplitudes, so their output is also `[-1, 1]`.
//! 3. [`unit()`] maps `[-1, 1]` to `[0, 1]`. It is the only mapping, and its clamp is a
//!    backstop that should never fire.
//!
//! Frequency is in cycles per block: callers pass block coordinates directly and the
//! frequency multiplies before sampling. Octave `i` samples seed `seed.wrapping_add(i)`
//! at `frequency * lacunarity^i`, which is the same octave decorrelation the existing
//! `src/generation.rs` gets from `fastnoise-lite`'s own fractal types.
//!
//! `fastnoise-lite` is seeded per call: [`FastNoiseLite::with_seed`] owns the whole
//! state, so a cached instance per seed is a pure function of `(seed, x, z)`. The
//! thread-local cache only avoids rebuilding the configuration struct per sample.

use std::cell::RefCell;

use fastnoise_lite::{FastNoiseLite, NoiseType};

use crate::description::MACRO_CELL;

/// Neutral value of a `[0, 1]` field, and the constant `unit(0)` produces.
pub const NEUTRAL: f32 = 0.5;

/// Measured maximum of the normalized `OpenSimplex2` 2D sampler over the sweep in
/// `tests::measure_noise_bounds`.
///
/// Changing the sampler, its hash, or its gradient table means re-measuring this value,
/// bumping the generator version, and updating the pinned tests.
pub const MEASURED_MAX_2D: f32 = 0.999_993_44;

/// Measured maximum of the normalized `OpenSimplex2` 3D sampler over the same sweep.
pub const MEASURED_MAX_3D: f32 = 0.999_801_9;

/// Divisor applied to every 2D sample: the measured maximum plus a 2% margin.
pub const NOISE_BOUND_2D: f32 = MEASURED_MAX_2D * 1.02;

/// Divisor applied to every 3D sample: the measured maximum plus a 2% margin.
pub const NOISE_BOUND_3D: f32 = MEASURED_MAX_3D * 1.02;

/// Maps a signed raw value to `[0, 1]`.
///
/// `unit(0) == 0.5`. The clamp is a backstop for rounding at the measured bound, never
/// a renormalisation step.
#[must_use]
pub fn unit(raw: f32) -> f32 {
    (raw * 0.5 + 0.5).clamp(0.0, 1.0)
}

thread_local! {
    /// One configured sampler per seed. Rebuilding it per sample would cost more than
    /// the sample itself; caching is invisible to the output because the configuration
    /// is a pure function of the seed.
    static CACHED_SAMPLER: RefCell<Option<(u32, FastNoiseLite)>> = const { RefCell::new(None) };
}

/// Runs `sample` against a sampler configured for `seed`.
fn with_sampler(seed: u32, sample: impl FnOnce(&FastNoiseLite) -> f32) -> f32 {
    CACHED_SAMPLER.with(|cached| {
        let mut cached = cached.borrow_mut();
        let matches = cached
            .as_ref()
            .is_some_and(|(cached_seed, _)| *cached_seed == seed);
        if !matches {
            let mut noise = FastNoiseLite::with_seed(seed.cast_signed());
            noise.set_noise_type(Some(NoiseType::OpenSimplex2));
            // The caller supplies block coordinates, so the frequency multiplier must
            // not add a second scale on top.
            noise.set_frequency(Some(1.0));
            *cached = Some((seed, noise));
        }
        let (_, noise) = cached
            .as_ref()
            .expect("the sampler was just inserted when no seed matched");
        sample(noise)
    })
}

/// One raw `OpenSimplex2` 2D sample at `(x, z)`, scaled into `[-1, 1]`.
#[must_use]
pub fn simplex2_2d(seed: u32, x: f32, z: f32) -> f32 {
    let raw = with_sampler(seed, |noise| noise.get_noise_2d(x, z));
    debug_assert!(raw.abs() <= 1.0, "OpenSimplex2 2D left [-1, 1]: {raw}");
    raw * (1.0 / NOISE_BOUND_2D)
}

/// One raw `OpenSimplex2` 3D sample at `(x, y, z)`, scaled into `[-1, 1]`.
#[must_use]
pub fn simplex2_3d(seed: u32, x: f32, y: f32, z: f32) -> f32 {
    let raw = with_sampler(seed, |noise| noise.get_noise_3d(x, y, z));
    debug_assert!(raw.abs() <= 1.0, "OpenSimplex2 3D left [-1, 1]: {raw}");
    raw * (1.0 / NOISE_BOUND_3D)
}

/// Sum of the amplitudes of `octaves` octaves with the given gain.
fn amplitude_sum(octaves: u32, gain: f32) -> f32 {
    let mut sum = 0.0;
    let mut amplitude = 1.0;
    for _ in 0..octaves {
        sum += amplitude;
        amplitude *= gain;
    }
    sum
}

/// Fixed-order fractal sum. `transform` maps one octave in `[-1, 1]` to `[-1, 1]`.
#[allow(clippy::too_many_arguments)]
fn fractal_2d(
    seed: u32,
    x: f32,
    z: f32,
    frequency: f32,
    octaves: u32,
    lacunarity: f32,
    gain: f32,
    transform: fn(f32) -> f32,
) -> f32 {
    let divisor = amplitude_sum(octaves, gain);
    if divisor <= 0.0 {
        return 0.0;
    }
    let mut total = 0.0;
    let mut amplitude = 1.0;
    let mut frequency = frequency;
    for octave in 0..octaves {
        let sample = simplex2_2d(seed.wrapping_add(octave), x * frequency, z * frequency);
        total += amplitude * transform(sample);
        amplitude *= gain;
        frequency *= lacunarity;
    }
    (total / divisor).clamp(-1.0, 1.0)
}

/// Fractal Brownian motion, `[-1, 1]`.
#[must_use]
pub fn fbm_2d(
    seed: u32,
    x: f32,
    z: f32,
    frequency: f32,
    octaves: u32,
    lacunarity: f32,
    gain: f32,
) -> f32 {
    fractal_2d(seed, x, z, frequency, octaves, lacunarity, gain, |sample| {
        sample
    })
}

/// Ridged fractal, `[-1, 1]`. Each octave is `1 - 2 * |n|`, so ridges sit at `+1`.
#[must_use]
pub fn ridged_2d(
    seed: u32,
    x: f32,
    z: f32,
    frequency: f32,
    octaves: u32,
    lacunarity: f32,
    gain: f32,
) -> f32 {
    fractal_2d(seed, x, z, frequency, octaves, lacunarity, gain, |sample| {
        1.0 - 2.0 * sample.abs()
    })
}

/// Billow fractal, `[-1, 1]`. Each octave is `2 * |n| - 1`.
#[must_use]
pub fn billow_2d(
    seed: u32,
    x: f32,
    z: f32,
    frequency: f32,
    octaves: u32,
    lacunarity: f32,
    gain: f32,
) -> f32 {
    fractal_2d(seed, x, z, frequency, octaves, lacunarity, gain, |sample| {
        2.0 * sample.abs() - 1.0
    })
}

/// One 2D domain warp: two `OpenSimplex2` taps producing a `(dx, dz)` offset in blocks.
///
/// Both taps run on raw signed values at the same frequency; the caller samples the
/// warped field at `(x + dx, z + dz)`. `amplitude` bounds each offset to `±amplitude`.
#[must_use]
pub fn warp_2d(
    seed_a: u32,
    seed_b: u32,
    x: f32,
    z: f32,
    frequency: f32,
    amplitude: f32,
) -> (f32, f32) {
    let warp_x = simplex2_2d(seed_a, x * frequency, z * frequency) * amplitude;
    let warp_z = simplex2_2d(seed_b, x * frequency, z * frequency) * amplitude;
    (warp_x, warp_z)
}

/// Halo cells a warp forces on a tile: `ceil(amplitude / MACRO_CELL) + 1`.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
#[must_use]
pub fn warp_halo_cells(amplitude: f32) -> i32 {
    let cells = (amplitude / MACRO_CELL as f32).ceil().max(0.0);
    cells as i32 + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::{domain, hash01, hash2, hash3};

    /// Coordinate span of the measurement and drift sweeps, in blocks.
    const SWEEP_SPAN: f32 = 4096.0;

    fn coordinate(seed: u32, index: i32, axis: i32) -> f32 {
        let tag = domain("w2-noise-sweep");
        let hash = hash3(u64::from(seed), tag, index, axis, -7);
        hash01(hash) * SWEEP_SPAN - SWEEP_SPAN * 0.5
    }

    #[test]
    fn unit_is_clamped_and_centred() {
        assert_eq!(unit(0.0), NEUTRAL);
        assert_eq!(unit(-1.0), 0.0);
        assert_eq!(unit(1.0), 1.0);
        assert_eq!(unit(-2.0), 0.0);
        assert_eq!(unit(2.0), 1.0);
        assert_eq!(unit(-1000.0), 0.0);
        assert_eq!(unit(1000.0), 1.0);
        for raw in [-1.0, -0.5, 0.25, 0.75, 1.0] {
            let mapped = unit(raw);
            assert!((0.0..=1.0).contains(&mapped));
        }
    }

    #[test]
    fn noise_stays_within_bound() {
        for seed in [0_u32, 1, 7, 0x5EED, 0xDEAD_BEEF] {
            for index in 0..20_000 {
                let x = coordinate(seed, index, 0);
                let z = coordinate(seed, index, 1);
                let y = coordinate(seed, index, 2);
                assert!(simplex2_2d(seed, x, z).abs() <= 1.0, "2D seed {seed}");
                assert!(
                    simplex2_3d(seed, x, y, z).abs() <= 1.0,
                    "3D seed {seed} at {x} {y} {z}"
                );
            }
        }
    }

    #[test]
    fn fractal_types_stay_within_bound() {
        for seed in [0_u32, 7, 0x5EED] {
            for index in (0..5000).step_by(3) {
                let x = coordinate(seed, index, 0);
                let z = coordinate(seed, index, 1);
                for value in [
                    fbm_2d(seed, x, z, 1.0 / 64.0, 5, 2.0, 0.5),
                    ridged_2d(seed, x, z, 1.0 / 64.0, 5, 2.0, 0.5),
                    billow_2d(seed, x, z, 1.0 / 64.0, 5, 2.0, 0.5),
                ] {
                    assert!(value.abs() <= 1.0, "{value} at {x} {z}");
                }
            }
        }
    }

    #[test]
    fn fbm_is_deterministic() {
        let first = fbm_2d(0x5EED, 12.5, -31.25, 1.0 / 64.0, 4, 2.0, 0.5);
        let second = fbm_2d(0x5EED, 12.5, -31.25, 1.0 / 64.0, 4, 2.0, 0.5);
        assert_eq!(first, second);
        assert_eq!(
            first.to_bits(),
            FBM_PINNED.to_bits(),
            "pinned fBm sample; a change here is a generator version bump"
        );
    }

    /// Pinned value of `fbm_2d(0x5EED, 12.5, -31.25, 1.0 / 64.0, 4, 2.0, 0.5)`.
    const FBM_PINNED: f32 = f32::from_bits(0xBD68_8ECD);

    #[test]
    fn noise_bound_is_pinned() {
        assert_eq!(NOISE_BOUND_2D, MEASURED_MAX_2D * 1.02);
        assert_eq!(NOISE_BOUND_3D, MEASURED_MAX_3D * 1.02);

        let mut max_2d = 0.0_f32;
        let mut max_3d = 0.0_f32;
        for seed in [7_u32, 0x5EED] {
            for index in 0..20_000 {
                let x = coordinate(seed, index, 0);
                let z = coordinate(seed, index, 1);
                let y = coordinate(seed, index, 2);
                let scaled_2d = simplex2_2d(seed, x, z).abs() * NOISE_BOUND_2D;
                let scaled_3d = simplex2_3d(seed, x, y, z).abs() * NOISE_BOUND_3D;
                max_2d = max_2d.max(scaled_2d);
                max_3d = max_3d.max(scaled_3d);
            }
        }
        assert!(
            max_2d <= MEASURED_MAX_2D,
            "2D drift: measured {max_2d} exceeds pinned {MEASURED_MAX_2D}"
        );
        assert!(
            max_3d <= MEASURED_MAX_3D,
            "3D drift: measured {max_3d} exceeds pinned {MEASURED_MAX_3D}"
        );
    }

    #[test]
    fn warp_offsets_stay_inside_the_amplitude() {
        for index in 0..10_000 {
            let x = coordinate(11, index, 0);
            let z = coordinate(11, index, 1);
            let (dx, dz) = warp_2d(21, 22, x, z, 1.0 / 96.0, 8.0);
            assert!(dx.abs() <= 8.0, "{dx}");
            assert!(dz.abs() <= 8.0, "{dz}");
        }
        assert_eq!(warp_halo_cells(0.0), 1);
        assert_eq!(warp_halo_cells(4.0), 2);
        assert_eq!(warp_halo_cells(5.0), 3);
        assert_eq!(warp_halo_cells(8.0), 3);
    }

    #[test]
    fn octaves_are_decorrelated_and_deterministic() {
        let tag = domain("w2-noise-octave");
        let x = hash01(hash2(3, tag, 5, 6)) * 100.0;
        let z = hash01(hash2(3, tag, 7, 8)) * 100.0;
        assert_eq!(simplex2_2d(3, x, z), simplex2_2d(3, x, z));
        assert_ne!(simplex2_2d(3, x, z), simplex2_2d(4, x, z));
    }

    #[test]
    #[ignore = "millions of samples; run explicitly to re-measure the pinned bounds"]
    fn measure_noise_bounds() {
        let mut max_2d = 0.0_f32;
        let mut max_3d = 0.0_f32;
        for seed in [0_u32, 1, 7, 0x5EED, 0xDEAD_BEEF, 0x1234_5678] {
            for index in 0..1_000_000 {
                let x = coordinate(seed, index, 0);
                let z = coordinate(seed, index, 1);
                let y = coordinate(seed, index, 2);
                let raw_2d = (simplex2_2d(seed, x, z) * NOISE_BOUND_2D).abs();
                let raw_3d = (simplex2_3d(seed, x, y, z) * NOISE_BOUND_3D).abs();
                max_2d = max_2d.max(raw_2d);
                max_3d = max_3d.max(raw_3d);
            }
        }
        println!("MEASURED_MAX_2D = {max_2d}");
        println!("MEASURED_MAX_3D = {max_3d}");
    }
}
