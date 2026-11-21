//! Lattice climate fields, the macro height grid, and the generation stage
//! (`E1.1`, `E1.2`, `E2.1`-`E2.3`, `E3.3`, `E3.5`).

use crate::description::{Axis, MACRO_CELL, SEA};
use crate::error::{ErrorKind, Stage, TerrainError};
use crate::hash::{domain, hash01, hash2};
use crate::noise::{fbm_2d, unit};

use super::params::{ClimateParams, altitude, latitude};
use super::table::{BiomeTable, smoothstep01};

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
pub(super) const TEMP_NOISE_FREQ: f32 = 1.0 / 512.0;

/// Octaves of the temperature noise (`E1.1`).
pub(super) const TEMP_NOISE_OCTAVES: u32 = 3;

/// Frequency of the humidity noise, in cycles per block.
pub(super) const HUMIDITY_NOISE_FREQ: f32 = 1.0 / 512.0;

/// Octaves of the humidity noise.
pub(super) const HUMIDITY_NOISE_OCTAVES: u32 = 3;

/// Lacunarity between noise octaves.
pub(super) const NOISE_LACUNARITY: f32 = 2.0;

/// Gain between noise octaves.
pub(super) const NOISE_GAIN: f32 = 0.5;

/// Sea level in blocks, as `f32` (`C1.2`).
#[allow(clippy::cast_precision_loss)]
pub(super) const SEA_HEIGHT: f32 = SEA as f32;

/// One macro cell in blocks, as `f32` (`B4.1`).
#[allow(clippy::cast_precision_loss)]
pub(super) const MACRO_CELL_F: f32 = MACRO_CELL as f32;

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
pub(super) fn clamp_unit(value: f32, fires: &mut u64) -> f32 {
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

/// World position of the lattice sample for one global cell (`B4.5`).
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub(super) fn sample_position(global_cell: i32) -> f32 {
    (global_cell * MACRO_CELL) as f32 + 0.5
}

/// Derives the `u32` noise seed for one 64-bit stream.
pub(super) fn stream_seed(seed: u64, tag: u64) -> u32 {
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
