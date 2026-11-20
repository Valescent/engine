//! W4 - strata bands, faults, and `stone_at`.
//!
//! `G1.1`, `G1.2`, `G1.4`, `G2.1`-`G2.4`. One resolver decides which stone a solid
//! cell is made of: [`Strata::stone_at`]. Cave walls, cliff faces, span undersides,
//! slope stone, and ore host checks all call it. It takes coordinates only - there is
//! no face, exposure, or neighbour argument, so a block cannot be one material on a
//! wall and another on a floor.
//!
//! # The band coordinate
//!
//! A `y` block belongs to band
//! `floor_div(y + 0.5 + strata_warp(x, z) + fault(x, z), THICKNESS)`, and the band
//! table is indexed by that value modulo its length. The modulus is `rem_euclid`, so
//! negative `y` wraps instead of panicking or reading out of bounds.
//!
//! `strata_warp` is one fBm (two octaves, ~1/96 cycles per block) sampled on the
//! 4-block macro lattice and bilinearly interpolated to block centres, scaled by the
//! warp amplitude. `fault` is a piecewise-constant offset from a cell grid, one of
//! `0`, `+3`, or `-3` blocks, smoothstepped over four blocks. Both are 2D and cost one
//! evaluation per column (`G2.3`): [`Strata::column`] performs them once and
//! [`Strata::stone_in`] resolves any `y` in that column from the cached offset.
//! [`Strata::stone_at`] is the convenience path for a single block.
//!
//! # The deep floor
//!
//! Below `DEEP_Y` the stone family becomes one deep rock (`G1.4`). A four-block ragged
//! transition sits directly above the floor, driven by a per-block hash at thresholds
//! that fall as `y` rises, so the boundary is not a flat plane. Below the transition
//! no band index and no warp are evaluated at all.
//!
//! # What this pass does not do
//!
//! Faults move materials only, never height (`G2.4`). Strata never override the
//! surface stack (`G1.3`): the surface rules of `E4.5` place soil, bed, beach, snow,
//! and bedrock, and this module only answers for cells the surface stack left as
//! stone. The band table is data held by [`Strata`], not a global.
//!
//! # Determinism
//!
//! The warp and the fault are evaluated at block centres with
//! [`crate::lattice::sample_pos`]. Every operation is `+ - * /` in a fixed order: no
//! `sin`, `cos`, `pow`, `exp`, `ln`, no `mul_add`, no hash map, and no division that
//! can produce NaN. The three streams are keyed by fixed domain tags that are hashed
//! once at construction, so the same seed and table produce the same world on every
//! target and a column costs no domain hash.

use crate::description::{DEEP_Y, MACRO_CELL};
use crate::error::{ErrorKind, Stage, TerrainError};
use crate::hash::{domain, hash01, hash2, hash3};
use crate::lattice::{bilinear, floor_div, macro_cell, sample_pos};
use crate::noise::fbm_2d;
use crate::section::BlockId;

/// Frequency of the strata warp in cycles per block (`G1.1`).
const WARP_FREQ: f32 = 1.0 / 96.0;

/// Octaves of the strata warp (`G1.1`).
const WARP_OCTAVES: u32 = 2;

/// Lacunarity between warp octaves.
const WARP_LACUNARITY: f32 = 2.0;

/// Gain between warp octaves.
const WARP_GAIN: f32 = 0.5;

/// Blocks of ragged transition directly above [`DEEP_Y`] (`G1.4`).
const DEEP_TRANSITION: i32 = 4;

/// Largest fault offset in blocks (`G2.2`).
const FAULT_STEP: f32 = 3.0;

/// Width in blocks of a fault ramp (`G2.2`).
const FAULT_RAMP: i32 = 4;

/// Domain tag of the warp field (`G2.1`).
const WARP_DOMAIN: &str = "strata_warp";

/// Domain tag of the fault field (`G2.2`).
const FAULT_DOMAIN: &str = "strata_fault";

/// Domain tag of the deep-rock dither (`G1.4`).
const DEEP_DOMAIN: &str = "deep_dither";

/// The stone band table and the two deformation fields that index it.
///
/// The table is data: construction validates it once, and every query is a pure
/// function of `(seed, table, x, y, z)`. It is not a global, so two worlds can hold
/// different tables in the same process.
#[derive(Clone, Debug)]
pub struct Strata {
    /// World seed. Keys the band warp, the fault grid, and the deep dither.
    seed: u64,
    /// Frequency of the warp fBm in cycles per block (`G1.1`).
    warp_freq: f32,
    /// Peak warp displacement in blocks (`G1.1`).
    warp_amp: f32,
    /// Band thickness in blocks (`G1.1`). Always positive.
    thickness: i32,
    /// Fault cell edge in blocks (`G2.2`). Always positive.
    fault_cell: i32,
    /// Stone that replaces every band below [`DEEP_Y`] (`G1.4`).
    deep: BlockId,
    /// Band ids, indexed by `band_index mod len`. Never empty.
    bands: Vec<BlockId>,
    /// Domain tag of the warp stream, hashed once at construction.
    warp_tag: u64,
    /// Domain tag of the fault stream, hashed once at construction.
    fault_tag: u64,
    /// Domain tag of the deep dither, hashed once at construction.
    deep_tag: u64,
}

/// The two 2D deformation fields for one column, evaluated once (`G2.3`).
///
/// A whole vertical stack reuses this value; only [`crate::description::DEEP_Y`] and
/// the band arithmetic remain per block. [`Strata::column`] builds it.
#[derive(Clone, Copy, Debug)]
pub struct ColumnStrata {
    /// Warp plus fault in blocks, the `y` offset of this column's band coordinate.
    offset: f32,
}

impl ColumnStrata {
    /// The combined warp and fault offset of this column, in blocks.
    #[must_use]
    pub fn offset(&self) -> f32 {
        self.offset
    }
}

impl Strata {
    /// Builds a stone table and its two deformation streams.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::DescriptionInvalid`] when `bands` is empty, when
    /// `thickness` or `fault_cell` is not positive, or when `warp_amp` is not a
    /// positive finite number. A table that cannot be indexed is rejected at
    /// construction rather than at the first query.
    pub fn new(
        seed: u64,
        thickness: i32,
        warp_amp: f32,
        fault_cell: i32,
        deep: BlockId,
        bands: Vec<BlockId>,
    ) -> Result<Self, TerrainError> {
        if bands.is_empty() {
            return Err(invalid("STRATA_BANDS", "non-empty"));
        }
        if thickness <= 0 {
            return Err(invalid("STRATA_THICKNESS", "positive"));
        }
        if fault_cell <= 0 {
            return Err(invalid("FAULT_CELL", "positive"));
        }
        if !warp_amp.is_finite() || warp_amp <= 0.0 {
            return Err(invalid("STRATA_WARP_AMP", "positive"));
        }
        Ok(Self {
            seed,
            warp_freq: WARP_FREQ,
            warp_amp,
            thickness,
            fault_cell,
            deep,
            bands,
            warp_tag: domain(WARP_DOMAIN),
            fault_tag: domain(FAULT_DOMAIN),
            deep_tag: domain(DEEP_DOMAIN),
        })
    }

    /// The 2D deformation fields of one column (`G2.3`).
    ///
    /// The warp fBm, the fault grid, and their bilinear blends are evaluated exactly
    /// once here. Reuse the result for every `y` in the column with
    /// [`Strata::stone_in`].
    #[must_use]
    pub fn column(&self, x: i32, z: i32) -> ColumnStrata {
        ColumnStrata {
            offset: self.strata_warp(x, z) + self.fault(x, z),
        }
    }

    /// The warp plus fault offset of one column, in blocks (`G1.1`, `G2.2`).
    #[must_use]
    pub fn column_offset(&self, x: i32, z: i32) -> f32 {
        self.column(x, z).offset
    }

    /// Resolves the stone id at one block (`G1.2`).
    ///
    /// This is the only material resolver. It has no face parameter: the same
    /// coordinate always yields the same id, whether the cell is a cave wall, a cliff
    /// face, or subsoil.
    #[must_use]
    pub fn stone_at(&self, x: i32, y: i32, z: i32) -> BlockId {
        if let Some(id) = self.deep_at(x, y, z) {
            return id;
        }
        self.band(self.column(x, z).offset, y)
    }

    /// Resolves the stone id at `y` using a column whose 2D fields are already known.
    ///
    /// This is the same rule as [`Strata::stone_at`] with the per-column cost already
    /// paid (`G2.3`), so a full column can call it per block. It never disagrees with
    /// `stone_at`.
    #[must_use]
    pub fn stone_in(&self, column: ColumnStrata, x: i32, y: i32, z: i32) -> BlockId {
        if let Some(id) = self.deep_at(x, y, z) {
            return id;
        }
        self.band(column.offset, y)
    }

    /// Deep rock when this block is below the floor or caught by the dither.
    ///
    /// `None` means the block is in the band table. The dither is only consulted in
    /// the four blocks above the floor: at `DEEP_Y - 4` the threshold is `1.0`, so
    /// that block is always deep, and the odds of deep rock fall to a quarter at
    /// `DEEP_Y - 1`. [`DEEP_Y`] itself is never deep (`G1.4`).
    fn deep_at(&self, x: i32, y: i32, z: i32) -> Option<BlockId> {
        if y < DEEP_Y - DEEP_TRANSITION {
            return Some(self.deep);
        }
        if y < DEEP_Y {
            let dither = hash01(hash3(self.seed, self.deep_tag, x, y, z));
            if dither < deep_threshold(y) {
                return Some(self.deep);
            }
        }
        None
    }

    /// One band lookup at the column's offset.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss
    )]
    fn band(&self, offset: f32, y: i32) -> BlockId {
        // `G1.1`: the band coordinate is the block centre shifted by the column.
        let shifted = (y as f32 + 0.5 + offset).floor() as i32;
        let index = floor_div(shifted, self.thickness);
        let len = self.bands.len() as i32;
        self.bands[index.rem_euclid(len) as usize]
    }

    /// The warped band field of one column (`G2.1`).
    ///
    /// Four fBm taps on the 4-block lattice, blended to the block centre with the
    /// exact quarter weights of [`crate::lattice::macro_cell`].
    fn strata_warp(&self, x: i32, z: i32) -> f32 {
        let (cell_x, weight_x) = macro_cell(x);
        let (cell_z, weight_z) = macro_cell(z);
        let seed = stream_seed(self.seed, self.warp_tag);
        let c00 = self.warp_sample(seed, cell_x, cell_z);
        let c10 = self.warp_sample(seed, cell_x + 1, cell_z);
        let c01 = self.warp_sample(seed, cell_x, cell_z + 1);
        let c11 = self.warp_sample(seed, cell_x + 1, cell_z + 1);
        bilinear(c00, c10, c01, c11, weight_x, weight_z)
    }

    /// The warp fBm at one lattice cell, scaled to blocks.
    fn warp_sample(&self, seed: u32, cell_x: i32, cell_z: i32) -> f32 {
        let x = sample_pos(cell_x.saturating_mul(MACRO_CELL));
        let z = sample_pos(cell_z.saturating_mul(MACRO_CELL));
        fbm_2d(
            seed,
            x,
            z,
            self.warp_freq,
            WARP_OCTAVES,
            WARP_LACUNARITY,
            WARP_GAIN,
        ) * self.warp_amp
    }

    /// The fault field of one column (`G2.2`).
    ///
    /// The offset is constant inside a cell of the fault grid and smoothstepped across
    /// the four blocks centred on a cell boundary.
    fn fault(&self, x: i32, z: i32) -> f32 {
        let tag = self.fault_tag;
        let cell_x = floor_div(x, self.fault_cell);
        let cell_z = floor_div(z, self.fault_cell);
        let local_x = x.saturating_sub(cell_x.saturating_mul(self.fault_cell));
        let local_z = z.saturating_sub(cell_z.saturating_mul(self.fault_cell));
        let (x0, x1, weight_x) = ramp_pair(local_x, self.fault_cell);
        let (z0, z1, weight_z) = ramp_pair(local_z, self.fault_cell);
        let c00 = fault_value(self.seed, tag, cell_x + x0, cell_z + z0);
        let c10 = fault_value(self.seed, tag, cell_x + x1, cell_z + z0);
        let c01 = fault_value(self.seed, tag, cell_x + x0, cell_z + z1);
        let c11 = fault_value(self.seed, tag, cell_x + x1, cell_z + z1);
        bilinear(c00, c10, c01, c11, weight_x, weight_z)
    }
}

/// The description-invalid error this module raises.
fn invalid(key: &'static str, rule: &'static str) -> TerrainError {
    TerrainError::new(Stage::Strata, ErrorKind::DescriptionInvalid { key, rule })
}

/// Deep-rock threshold for one block inside the transition (`G1.4`).
///
/// [`DEEP_Y`] itself is not in the transition; the four blocks above the floor run
/// `1.0`, `0.75`, `0.5`, `0.25`. `hash01` is in `[0, 1)`, so `1.0` always passes.
#[allow(clippy::cast_precision_loss)]
fn deep_threshold(y: i32) -> f32 {
    let depth = (y - (DEEP_Y - DEEP_TRANSITION)) as f32;
    1.0 - depth * (1.0 / DEEP_TRANSITION as f32)
}

/// The `u32` noise seed of one named stream.
///
/// `fbm_2d` takes a `u32` seed, so the `u64` stream word is folded down. The `(0, 0)`
/// coordinate is a fixed anchor, never a world position: this is a stream tag, not a
/// sample. The tag is cached on the table, so this is one hash per column, not a
/// domain hash per column.
#[allow(clippy::cast_possible_truncation)]
fn stream_seed(seed: u64, tag: u64) -> u32 {
    let hash = hash2(seed, tag, 0, 0);
    (hash ^ (hash >> 32)) as u32
}

/// The fault offset of one cell: `0`, `+3`, or `-3` blocks (`G2.2`).
fn fault_value(seed: u64, tag: u64, cell_x: i32, cell_z: i32) -> f32 {
    match hash2(seed, tag, cell_x, cell_z) % 3 {
        0 => 0.0,
        1 => FAULT_STEP,
        _ => -FAULT_STEP,
    }
}

/// Which two cells an axis blends between, and the weight of the upper one.
///
/// The pair is `(-1, 0)` in the two blocks before a boundary, `(0, 1)` in the two
/// blocks after one, and `(0, 0)` with weight zero anywhere else. The returned weight
/// is already smoothstepped.
#[allow(clippy::cast_precision_loss)]
fn ramp_pair(local: i32, cell: i32) -> (i32, i32, f32) {
    let half = FAULT_RAMP / 2;
    let ramp = FAULT_RAMP as f32;
    if local < half {
        let t = (local as f32 + half as f32 + 0.5) / ramp;
        (-1, 0, smoothstep(t))
    } else if cell - local <= half {
        let t = (local as f32 + half as f32 + 0.5 - cell as f32) / ramp;
        (0, 1, smoothstep(t))
    } else {
        (0, 0, 0.0)
    }
}

/// The classic cubic smoothstep on `[0, 1]`.
fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Band id that is never the deep rock.
    const BAND_A: BlockId = 11;
    /// Second band id.
    const BAND_B: BlockId = 22;
    /// Third band id.
    const BAND_C: BlockId = 33;
    /// Deep rock, distinct from every band so a deep hit is visible.
    const DEEP: BlockId = 99;
    /// Band thickness of the fixture table.
    const THICKNESS: i32 = 24;
    /// Warp amplitude of the fixture table.
    const WARP_AMP: f32 = 8.0;
    /// Fault cell edge of the fixture table.
    const FAULT_CELL: i32 = 256;

    /// Spread of columns a test walks. Includes negatives and the world edge.
    const COLUMNS: [(i32, i32); 5] = [(0, 0), (37, -91), (-512, 640), (1023, 1023), (-3, 5)];

    /// Highest block of the world (`C4.3`).
    const WORLD_TOP: i32 = 383;

    /// The standard three-band table.
    fn table() -> Strata {
        table_with_seed(0x5EED)
    }

    /// The standard table at a chosen seed.
    fn table_with_seed(seed: u64) -> Strata {
        Strata::new(
            seed,
            THICKNESS,
            WARP_AMP,
            FAULT_CELL,
            DEEP,
            vec![BAND_A, BAND_B, BAND_C],
        )
        .expect("the fixture table is valid")
    }

    /// The band id the spec's arithmetic gives for one column at one `y`.
    ///
    /// Deliberately independent of the implementation: `floor_div`, then
    /// `rem_euclid`, then a plain array.
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    fn expected_band(column: ColumnStrata, y: i32) -> BlockId {
        const BANDS: [BlockId; 3] = [BAND_A, BAND_B, BAND_C];
        let shifted = (y as f32 + 0.5 + column.offset()).floor() as i32;
        let index = floor_div(shifted, THICKNESS).rem_euclid(3);
        BANDS[usize::try_from(index).expect("a remainder is never negative")]
    }

    /// Compares two `f32` bit patterns, so a test never relies on float equality.
    fn assert_bits(actual: f32, expected: f32) {
        assert_eq!(
            actual.to_bits(),
            expected.to_bits(),
            "{actual} and {expected} differ"
        );
    }

    /// The three contexts a face-aware material rule would distinguish.
    #[derive(Clone, Copy)]
    enum Face {
        /// A cave wall, with air in front of it.
        CaveWall,
        /// A cliff face, with air beside it.
        Cliff,
        /// Buried subsoil, solid on every side.
        Subsoil,
    }

    /// Resolves one cell in a context.
    ///
    /// A face-aware rule would branch here. v1 cannot: every arm reaches the same
    /// resolver with the same coordinates.
    fn face_stone(strata: &Strata, face: Face, x: i32, y: i32, z: i32) -> BlockId {
        match face {
            Face::CaveWall | Face::Cliff | Face::Subsoil => strata.stone_at(x, y, z),
        }
    }

    #[test]
    fn one_resolver_for_every_face() {
        let strata = table();

        // The signature is the contract: coordinates only, no face argument.
        let resolver: fn(&Strata, i32, i32, i32) -> BlockId = Strata::stone_at;

        for (x, z) in COLUMNS {
            let column = strata.column(x, z);
            for y in [DEEP_Y + 1, DEEP_Y + 25, 130, 191, 288, 383] {
                let expected = expected_band(column, y);
                assert_eq!(resolver(&strata, x, y, z), expected, "{x} {y} {z}");
                for face in [Face::CaveWall, Face::Cliff, Face::Subsoil] {
                    assert_eq!(
                        face_stone(&strata, face, x, y, z),
                        expected,
                        "a cave wall, a cliff, and subsoil must not differ at {x} {y} {z}"
                    );
                }
            }
        }
    }

    #[test]
    fn deep_rock_replaces_all_bands() {
        let strata = table();
        for (x, z) in COLUMNS {
            for y in (DEEP_Y - 260)..(DEEP_Y - DEEP_TRANSITION) {
                assert_eq!(strata.stone_at(x, y, z), DEEP, "{x} {y} {z}");
            }
        }
    }

    #[test]
    fn deep_transition_is_ragged_and_bounded() {
        let strata = table();
        let mut saw_deep = false;
        let mut saw_band = false;
        for (x, z) in COLUMNS {
            for y in (DEEP_Y - DEEP_TRANSITION)..=DEEP_Y {
                let id = strata.stone_at(x, y, z);
                saw_deep |= id == DEEP;
                saw_band |= id != DEEP;
            }
            // The bottom of the transition has threshold 1.0, so it is always deep.
            assert_eq!(strata.stone_at(x, DEEP_Y - DEEP_TRANSITION, z), DEEP);
            for y in (DEEP_Y + 1)..=(DEEP_Y + 64) {
                assert_ne!(strata.stone_at(x, y, z), DEEP, "{x} {y} {z}");
            }
        }
        assert!(saw_deep, "the transition never went deep");
        assert!(saw_band, "the transition never left deep rock");
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn bands_repeat_and_negative_y_is_safe() {
        let strata = table();
        for (x, z) in COLUMNS {
            let column = strata.column(x, z);
            for y in -260..=383 {
                let id = strata.stone_at(x, y, z);
                if y < DEEP_Y - DEEP_TRANSITION {
                    assert_eq!(id, DEEP, "{x} {y} {z}");
                } else if y < DEEP_Y {
                    // The dither owns these four blocks; the band path still applies
                    // wherever the dither does not fire.
                    assert_eq!(strata.stone_in(column, x, y, z), id);
                    assert!(id == DEEP || id == expected_band(column, y), "{x} {y} {z}");
                } else {
                    assert_eq!(id, expected_band(column, y), "{x} {y} {z}");
                }
            }
        }
    }

    #[test]
    fn fault_offset_is_piecewise_constant() {
        let cell = 64;
        let strata = Strata::new(11, THICKNESS, WARP_AMP, cell, DEEP, vec![BAND_A, BAND_B])
            .expect("the fixture table is valid");
        let z = cell / 2;

        // Find a cell boundary where the two neighbouring cells got different
        // offsets. They are hash-picked, so search rather than assume.
        let mut step = None;
        for k in 1..8 {
            let boundary = k * cell;
            let low = strata.fault(boundary - 3, z);
            let high = strata.fault(boundary + 2, z);
            if low.to_bits() != high.to_bits() {
                step = Some((boundary, low, high));
                break;
            }
        }
        let (boundary, low, high) = step.expect("adjacent fault cells must differ somewhere");

        // Offsets are pieces of the fixed {-3, 0, +3} table, so a step is at most 3.
        assert!((high - low).abs() <= FAULT_STEP, "{low} -> {high}");

        // Constant inside a cell: adjacent columns away from a boundary agree, and
        // they still agree several blocks away.
        assert_bits(strata.fault(boundary - 10, z), low);
        assert_bits(strata.fault(boundary - 9, z), low);
        assert_bits(strata.fault(boundary - 4, z), low);
        assert_bits(strata.fault(boundary + 2, z), high);
        assert_bits(strata.fault(boundary + 3, z), high);
        assert_bits(strata.fault(boundary + 10, z), high);

        // The step spans exactly the four-block ramp, and only there.
        let ramp: Vec<f32> = (-2..2)
            .map(|offset| strata.fault(boundary + offset, z))
            .collect();
        let (min, max) = if low < high { (low, high) } else { (high, low) };
        for value in &ramp {
            assert!(
                min < *value && *value < max,
                "a ramp block must sit between the two cell values: {value}"
            );
        }
        let rising = high > low;
        for pair in ramp.windows(2) {
            let monotone = if rising {
                pair[0] < pair[1]
            } else {
                pair[0] > pair[1]
            };
            assert!(monotone, "the ramp must be monotone: {pair:?}");
        }

        // `column_offset` is exactly the two 2D fields summed (`G2.3`).
        assert_bits(
            strata.column_offset(boundary - 10, z),
            strata.strata_warp(boundary - 10, z) + strata.fault(boundary - 10, z),
        );
    }

    #[test]
    fn column_offset_is_2d() {
        let strata = table();
        for (x, z) in COLUMNS {
            // Called once, then reused for the whole column.
            let column = strata.column(x, z);
            assert_bits(column.offset(), strata.column_offset(x, z));
            for y in [DEEP_Y, DEEP_Y + 7, 96, 150, 288, 383] {
                assert_eq!(
                    strata.stone_in(column, x, y, z),
                    strata.stone_at(x, y, z),
                    "a cached column must resolve every y like the per-block path at \
                     {x} {y} {z}"
                );
            }
        }
    }

    #[test]
    fn strata_is_deterministic() {
        let first = table();
        let second = table();
        for (x, z) in COLUMNS {
            assert_bits(first.column_offset(x, z), second.column_offset(x, z));
            for y in [DEEP_Y - 2, DEEP_Y, 100, 300] {
                assert_eq!(first.stone_at(x, y, z), second.stone_at(x, y, z));
            }
        }

        let other_seed = table_with_seed(0x5EEE);
        let changed = COLUMNS.iter().any(|&(x, z)| {
            first.column_offset(x, z).to_bits() != other_seed.column_offset(x, z).to_bits()
        });
        assert!(changed, "a seed change must move the 2D fields");
    }

    #[test]
    fn cached_domain_tags_do_not_change_output() {
        // Pinned before the tags were hoisted into `Strata::new`. These bit patterns
        // are the pre-cache output: if one moves, the change is a generator change,
        // not a performance fix.
        const PINNED: [(i32, i32, u32); 5] = [
            (0, 0, 0x3fb5_2874),
            (37, -91, 0x4082_5760),
            (-512, 640, 0xbdac_8040),
            (1023, 1023, 0xbeaf_2920),
            (-3, 5, 0xc08a_f5a0),
        ];
        let strata = table();
        for (x, z, bits) in PINNED {
            assert_eq!(strata.column_offset(x, z).to_bits(), bits, "{x} {z}");
        }

        // The deep dither reads a cached tag too, so pin ids across the transition.
        assert_eq!(strata.stone_at(37, DEEP_Y - 3, -91), DEEP);
        assert_eq!(strata.stone_at(-512, DEEP_Y - 3, 640), DEEP);
        assert_eq!(strata.stone_at(0, DEEP_Y - 3, 0), BAND_C);
        assert_eq!(strata.stone_at(0, DEEP_Y - 1, 0), BAND_C);
        assert_eq!(strata.stone_at(0, 100, 0), BAND_B);
    }

    #[test]
    fn stone_at_matches_stone_in() {
        let strata = table();

        // A few thousand cells: negatives, both ends of the world height, and the
        // whole deep transition. The column is rebuilt per call here on purpose, so
        // the pairing itself is what is exercised.
        for index in 0..512 {
            let x = (index * 37) % 1024 - 512;
            let z = (index * 91) % 768 - 384;
            for y in [
                -1,
                0,
                DEEP_Y - DEEP_TRANSITION - 1,
                DEEP_Y - DEEP_TRANSITION,
                DEEP_Y - 2,
                DEEP_Y - 1,
                DEEP_Y,
                DEEP_Y + 1,
                128,
                WORLD_TOP,
            ] {
                assert_eq!(
                    strata.stone_at(x, y, z),
                    strata.stone_in(strata.column(x, z), x, y, z),
                    "{x} {y} {z}"
                );
            }
        }

        // Every layer of a handful of columns, so nothing between the samples can
        // drift either.
        for (x, z) in COLUMNS {
            let column = strata.column(x, z);
            for y in -8..=WORLD_TOP {
                assert_eq!(
                    strata.stone_at(x, y, z),
                    strata.stone_in(column, x, y, z),
                    "{x} {y} {z}"
                );
            }
        }
    }

    #[test]
    fn bad_tables_are_rejected() {
        let cases = [
            Strata::new(1, THICKNESS, WARP_AMP, FAULT_CELL, DEEP, Vec::new()),
            Strata::new(1, 0, WARP_AMP, FAULT_CELL, DEEP, vec![BAND_A]),
            Strata::new(1, -4, WARP_AMP, FAULT_CELL, DEEP, vec![BAND_A]),
            Strata::new(1, THICKNESS, WARP_AMP, 0, DEEP, vec![BAND_A]),
            Strata::new(1, THICKNESS, WARP_AMP, -8, DEEP, vec![BAND_A]),
            Strata::new(1, THICKNESS, 0.0, FAULT_CELL, DEEP, vec![BAND_A]),
            Strata::new(1, THICKNESS, -1.0, FAULT_CELL, DEEP, vec![BAND_A]),
            Strata::new(1, THICKNESS, f32::NAN, FAULT_CELL, DEEP, vec![BAND_A]),
        ];
        for result in cases {
            let Err(error) = result else {
                panic!("a table that cannot be indexed was accepted");
            };
            assert!(
                matches!(error.kind, ErrorKind::DescriptionInvalid { .. }),
                "{error:?}"
            );
        }
    }
}
