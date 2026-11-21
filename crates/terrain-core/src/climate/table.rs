//! Biome centres, nearest-two selection, and the normalised border quantiser
//! (`E3.1`, `E3.3`, `E3.5`).

use std::cmp::Ordering;

use crate::error::TerrainError;

use super::params::invalid;

/// Scale applied to a temperature delta before squaring (`E3.1`).
///
/// Both climate axes are `[0, 1]`, so the neutral scale keeps climate space a unit
/// square and a biome centre is directly comparable to a coordinate.
pub const T_SCALE: f32 = 1.0;

/// Scale applied to a humidity delta before squaring (`E3.1`).
pub const H_SCALE: f32 = 1.0;

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
