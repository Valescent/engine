//! Cave carving, the entrance field, roofs, and the two cave families.

use crate::description::{CAVE_MIN_ROOF, MACRO_CELL};
use crate::noise::{fbm_2d, simplex2_3d, unit};
use crate::section::BlockId;

use super::{COLUMN_HEIGHT, ColumnInput, Volume, VolumeStats, WORLD_LAST_Y, ramp};

/// Noise taps a single 3D cave sample may spend (`F2.1`).
///
/// Two cheese octaves plus two spaghetti fields. The counter in [`CaveSample`] is
/// the assertion: a sample that spent more would be a version bump.
pub(crate) const CAVE_TAP_BUDGET: u8 = 4;

/// Integer slope drop at which a column may open a cave mouth (`F2.5`).
pub(crate) const MOUTH_SLOPE_MIN: i32 = 2;

/// Clearance, in blocks, that keeps a mouth off a river or lake bank (`F2.5`).
pub(crate) const BANK_CLEARANCE: i32 = 2;

/// Frequency of the cheese fBm, in cycles per block (`F2.1`).
const CHEESE_FREQ: f32 = 1.0 / 48.0;

/// Frequency of the spaghetti fields, in cycles per block (`F2.1`).
const SPAGHETTI_FREQ: f32 = 1.0 / 64.0;

/// Frequency of the entrance field, in cycles per block (`F2.5`).
const ENTRANCE_FREQ: f32 = 1.0 / 192.0;

/// Noise distance from neutral at which a spaghetti tunnel closes (`F2.1`).
///
/// The carve threshold is `cave_t`, so the surviving tunnel is roughly the inner
/// half of this band. At `1/64` and the default threshold that is a radius of
/// several blocks; `tunnel_radius_at_least_three_blocks` measures it.
const SPAGHETTI_RADIUS: f32 = 0.36;

// Cheese depth window (`F2.3`): `d = top_solid_y - y`.
/// Cheese is absent above this many blocks below the top.
const CHEESE_DEPTH_START: i32 = 12;
/// Cheese reaches full weight at this depth.
pub(super) const CHEESE_DEPTH_FULL: i32 = 40;
/// Full weight is held to this depth.
pub(super) const CHEESE_DEPTH_FLAT_END: i32 = 140;
/// Cheese is absent from this depth down.
pub(super) const CHEESE_DEPTH_END: i32 = 200;

// Cheese floor window (`F2.3`): absolute `y`.
/// Cheese is absent below this `y`.
pub(super) const CHEESE_FLOOR_START: i32 = 8;
/// Cheese reaches full weight at this `y`.
pub(super) const CHEESE_FLOOR_FULL: i32 = 24;

/// Lowest block either cave family may carve (`F2.3`: weights are zero at
/// `y = 0..7`).
///
/// The lattice weights are sampled at `4k + 0.5`, so interpolation alone lets a
/// block at `y = 7` borrow a quarter of the cell at `y = 8.5`. This hard gate is
/// what makes the stated block-level floor exact.
const CAVE_FLOOR_Y: i32 = if CHEESE_FLOOR_START < SPAGHETTI_START_Y {
    CHEESE_FLOOR_START
} else {
    SPAGHETTI_START_Y
};

/// Blocks below a near-sea top where cheese is forced to zero (`F2.3`).
pub(super) const CHEESE_SEAL_BLOCKS: i32 = 24;

// Spaghetti `y` window (`F2.3`).
/// Spaghetti is absent below this `y`.
pub(super) const SPAGHETTI_START_Y: i32 = 12;
/// Spaghetti reaches full weight at this `y`.
pub(super) const SPAGHETTI_FULL_Y: i32 = 24;
/// Full weight is held to this `y`.
pub(super) const SPAGHETTI_FLAT_END_Y: i32 = 296;
/// Spaghetti is absent from this `y` up.
pub(super) const SPAGHETTI_END_Y: i32 = 308;

// Spaghetti depth window (`F2.3`).
/// Spaghetti is absent closer than this to the top.
const SPAGHETTI_DEPTH_START: i32 = 4;
/// Spaghetti reaches full weight at this depth.
pub(super) const SPAGHETTI_DEPTH_FULL: i32 = 16;

/// A lowland top (`top_solid_y <= SEA + this`) uses the capped ceiling.
const LOWLAND_TOP_OFFSET: i32 = 8;

/// Lowland spaghetti ceiling, in blocks below sea (`SEA - 8`).
pub(super) const LOWLAND_CEILING_DEPTH: i32 = 8;

/// Width of the entrance ramp above `entrance_t` (`F2.5`).
pub(super) const ENTRANCE_RAMP: f32 = 0.05;

/// Threshold reduction at a fully open mouth (`F2.5`, "eases the density threshold").
const MOUTH_EASE: f32 = 0.25;

/// Entrance curve: x is the raw field in `[0, 1]` (`F2.5`).
///
/// The curve is what makes `entrance_t = 0.82` a rare event on a field whose
/// `unit()` value is centred near 0.5.
pub(super) const ENTRANCE_KNOTS: &[(f32, f32)] = &[
    (0.0, 0.0),
    (0.40, 0.08),
    (0.55, 0.35),
    (0.65, 0.72),
    (0.72, 0.90),
    (0.80, 0.97),
    (1.0, 1.0),
];

/// One block span of lattice cells a column may skip (`F1.3`).
///
/// Both ends are world block `y`, inclusive. [`skippable_band`] returns the span
/// above the cave roof; everything below `y = 1` is skipped as well, because the
/// bedrock block at `y = 0` is never carved.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) struct SpanSkip {
    /// First block `y` that is never sampled.
    pub y_min: i32,
    /// Last block `y` that is never sampled.
    pub y_max: i32,
}

/// The block band above a column's cave roof that needs no lattice sample (`F1.3`).
///
/// A lattice cell is skipped when its whole four-block neighbourhood sits above
/// `top - CAVE_MIN_ROOF`, which is exactly `y >= y_min`; the `y = 0` cell is *not*
/// skipped because it also holds `y = 1..=3`.
#[must_use]
pub(crate) fn skippable_band(top: i32) -> SpanSkip {
    SpanSkip {
        y_min: top.saturating_sub(CAVE_MIN_ROOF).saturating_add(1),
        y_max: WORLD_LAST_Y,
    }
}

/// Whether the lattice cell containing blocks `4c ..= 4c + 3` is skipped (`F1.3`).
///
/// `top` is the column's highest solid block after the detail pass. A cell is
/// skipped when its four-block neighbourhood is entirely above the cave roof or
/// entirely below `CAVE_FLOOR_Y` (the `F2.3` floor, `y = 8`).
#[must_use]
pub(crate) fn cell_skipped(cell: i32, top: i32) -> bool {
    let first = cell.saturating_mul(MACRO_CELL);
    first >= skippable_band(top).y_min || first.saturating_add(MACRO_CELL - 1) < CAVE_FLOOR_Y
}

/// The lattice cell of one block coordinate and its exact quarter weight (`B4.5`).
///
/// This is the `y` axis of [`crate::lattice::macro_cell`] and must agree with it
/// bit for bit: `(floor_div(y, 4), rem_euclid(y, 4) / 4)`. A test pins the two
/// together on both sides of the origin.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub(crate) fn lattice_cell(y: i32) -> (i32, f32) {
    let cell = y.div_euclid(MACRO_CELL);
    let weight = y.rem_euclid(MACRO_CELL) as f32 / MACRO_CELL as f32;
    (cell, weight)
}

/// The two densities of one 3D cave sample and the taps it cost (`F2.1`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CaveSample {
    /// `max(cheese, spaghetti)`, in `[0, 1]`.
    pub density: f32,
    /// Noise taps spent: always [`CAVE_TAP_BUDGET`].
    pub taps: u8,
    /// Whether the final clamp had to fire. The contract is that it never does.
    pub clamped: bool,
}

impl Volume {
    /// The entrance field at one column, in `[0, 1]` (`F2.5`).
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn entrance(&self, x: i32, z: i32) -> f32 {
        let raw = fbm_2d(
            self.entrance_seed,
            x as f32,
            z as f32,
            ENTRANCE_FREQ,
            2,
            2.0,
            0.5,
        );
        self.entrance_curve.sample(unit(raw))
    }

    /// Whether the entrance field flags this column (`F2.5`).
    ///
    /// This is the coordinate half of the mouth test: the field crossing
    /// `entrance_t`. Slope, height, and the river/lake/bank exclusions live in
    /// [`Volume::mouth_open`], because they are column facts rather than field
    /// facts. A caller that keeps the flag for the span pass wants
    /// `is_mouth(x, z) && input.slope >= MOUTH_SLOPE_MIN`, or simply the value
    /// [`Volume::mouth_open`] returns.
    #[must_use]
    pub(crate) fn is_mouth(&self, x: i32, z: i32) -> bool {
        self.entrance(x, z) >= self.params.entrance_t
    }

    /// The complete `F2.5` mouth gate for one column.
    ///
    /// True when the entrance field is high **and** the column has the required
    /// slope **and** none of the hard exclusions applies. The exclusions are never
    /// overridden by the mask: a flat column, a column at or below `SEA + 1`, a
    /// river bed, a lake bed, and a bank all keep the default roof.
    #[must_use]
    pub(crate) fn mouth_open(&self, input: &ColumnInput) -> bool {
        self.is_mouth(input.x, input.z)
            && input.slope >= MOUTH_SLOPE_MIN
            && input.top > self.params.sea + 1
            && !input.is_river
            && !input.is_lake
            && !input.near_bank
    }

    /// The roof thickness in blocks for one column (`F2.5`).
    ///
    /// [`crate::description::CAVE_MIN_ROOF`] unless [`Volume::mouth_open`] holds, in
    /// which case the entrance ramp lowers it towards zero.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn roof(&self, input: &ColumnInput) -> f32 {
        let default = self.params.cave_min_roof as f32;
        if !self.mouth_open(input) {
            return default;
        }
        lerp(default, 0.0, self.entrance_ease(input))
    }

    /// The two cave densities at one 3D position, and their tap cost (`F2.1`).
    ///
    /// `x`, `y`, and `z` are world sample positions, not block indices; a lattice
    /// sample passes `4k + 0.5` on every axis. `top` is the column's
    /// `HeightTile` top; the `F2.3` depth weights are a function of `(top, y)`
    /// and therefore cost no extra tap.
    #[must_use]
    pub(crate) fn cave_density(&self, x: f32, y: f32, z: f32, top: i32) -> CaveSample {
        let cheese = self.cheese(x, y, z, top);
        let spaghetti = self.spaghetti(x, y, z, top);
        let density = cheese.max(spaghetti);
        CaveSample {
            density: density.clamp(0.0, 1.0),
            taps: CAVE_TAP_BUDGET,
            clamped: !(0.0..=1.0).contains(&density),
        }
    }

    /// `F1.5` step 4: carve cheese and spaghetti, roof-gated and bedrock-safe.
    pub(super) fn cave_pass(
        &self,
        column: &mut [BlockId; COLUMN_HEIGHT],
        input: &ColumnInput,
        air: BlockId,
        stats: &mut VolumeStats,
    ) {
        let Some(solid_top) = highest_solid(column, air) else {
            return;
        };
        let mouth = self.mouth_open(input);
        if mouth {
            stats.mouths += 1;
        }
        #[allow(clippy::cast_precision_loss)]
        let roof = self.roof(input);
        let threshold = self.threshold(input);
        let mut lattice = LatticeColumn::new(self, input.x, input.z, input.top);
        for y in CAVE_FLOOR_Y.max(1)..=solid_top {
            #[allow(clippy::cast_precision_loss)]
            let clearance = (solid_top - y) as f32;
            if clearance < roof {
                continue;
            }
            let (cell, weight) = lattice_cell(y);
            let near = lattice.density(cell);
            let density = if weight > 0.0 {
                let far = lattice.next_density();
                near + (far - near) * weight
            } else {
                near
            };
            let index = usize::try_from(y).unwrap_or(0);
            if density > threshold && column[index] != air {
                column[index] = air;
                stats.cave_cells += 1;
            }
        }
        stats.taps = stats.taps.saturating_add(lattice.taps);
    }

    /// How far up the entrance ramp this column sits, in `[0, 1]` (`F2.5`).
    fn entrance_ease(&self, input: &ColumnInput) -> f32 {
        let field = self.entrance(input.x, input.z);
        smoothstep(
            self.params.entrance_t,
            self.params.entrance_t + ENTRANCE_RAMP,
            field,
        )
    }

    /// The density threshold for one column (`F2.5`).
    fn threshold(&self, input: &ColumnInput) -> f32 {
        if !self.mouth_open(input) {
            return self.params.cave_t;
        }
        (self.params.cave_t - MOUTH_EASE * self.entrance_ease(input)).max(0.0)
    }

    /// Cheese: two-octave fBm at `1/48`, scaled by the `F2.3` windows.
    #[allow(clippy::cast_precision_loss)]
    pub(super) fn cheese(&self, x: f32, y: f32, z: f32, top: i32) -> f32 {
        let frequency = CHEESE_FREQ;
        let first = simplex2_3d(
            self.cheese_seeds[0],
            x * frequency,
            y * frequency,
            z * frequency,
        );
        let second = simplex2_3d(
            self.cheese_seeds[1],
            x * frequency * 2.0,
            y * frequency * 2.0,
            z * frequency * 2.0,
        );
        let fbm = (first + 0.5 * second) * (1.0 / 1.5);
        unit(fbm) * self.cheese_weight(top, y)
    }

    /// Spaghetti: two single-octave fields whose zero set is a tunnel (`F2.1`).
    #[allow(clippy::many_single_char_names)]
    pub(super) fn spaghetti(&self, x: f32, y: f32, z: f32, top: i32) -> f32 {
        let frequency = SPAGHETTI_FREQ;
        let a = unit(simplex2_3d(
            self.spaghetti_seeds[0],
            x * frequency,
            y * frequency,
            z * frequency,
        ));
        let b = unit(simplex2_3d(
            self.spaghetti_seeds[1],
            x * frequency,
            y * frequency,
            z * frequency,
        ));
        let distance = ((a - 0.5).abs() * 2.0).max((b - 0.5).abs() * 2.0);
        let tunnel = 1.0 - smooth01(distance / SPAGHETTI_RADIUS);
        tunnel * self.spaghetti_weight(top, y)
    }

    /// The `F2.3` cheese weight at one `(top, y)`.
    ///
    /// Product of the depth trapezoid, the absolute-`y` floor, and the
    /// near-sea seal. All three are monotone factors; the seal is a hard zero
    /// because a beach or sea floor within 24 blocks of its top must stay solid.
    #[allow(clippy::cast_precision_loss)]
    pub(super) fn cheese_weight(&self, top: i32, y: f32) -> f32 {
        if top <= self.params.sea.saturating_add(1)
            && y > top.saturating_sub(CHEESE_SEAL_BLOCKS) as f32
        {
            return 0.0;
        }
        let depth = top as f32 - y;
        cheese_depth_weight(depth) * cheese_floor_weight(y)
    }

    /// The `F2.3` spaghetti weight at one `(top, y)`.
    ///
    /// Product of the `y` trapezoid and the depth ramp. A lowland top
    /// (`top <= SEA + 8`) caps the ceiling at `SEA - 8`, so a coastal plain has
    /// no shallow tunnels; a highland column keeps the full 296..308 taper.
    #[allow(clippy::cast_precision_loss)]
    fn spaghetti_weight(&self, top: i32, y: f32) -> f32 {
        let ceiling = self.params.sea - LOWLAND_CEILING_DEPTH;
        if top <= self.params.sea + LOWLAND_TOP_OFFSET && y > ceiling as f32 {
            return 0.0;
        }
        let depth = top as f32 - y;
        spaghetti_y_weight(y) * spaghetti_depth_weight(depth)
    }
}

/// Cheese depth trapezoid: `0` below 12, `1` by 40, flat to 140, `0` by 200.
#[allow(clippy::cast_precision_loss)]
pub(super) fn cheese_depth_weight(depth: f32) -> f32 {
    ramp(CHEESE_DEPTH_START as f32, CHEESE_DEPTH_FULL as f32, depth)
        * (1.0 - ramp(CHEESE_DEPTH_FLAT_END as f32, CHEESE_DEPTH_END as f32, depth))
}

/// Cheese floor ramp: `0` below `y = 8`, `1` by `y = 24`.
#[allow(clippy::cast_precision_loss)]
pub(super) fn cheese_floor_weight(y: f32) -> f32 {
    ramp(CHEESE_FLOOR_START as f32, CHEESE_FLOOR_FULL as f32, y)
}

/// Spaghetti `y` trapezoid: `0` below 12, `1` from 24, flat to 296, `0` by 308.
#[allow(clippy::cast_precision_loss)]
pub(super) fn spaghetti_y_weight(y: f32) -> f32 {
    ramp(SPAGHETTI_START_Y as f32, SPAGHETTI_FULL_Y as f32, y)
        * (1.0 - ramp(SPAGHETTI_FLAT_END_Y as f32, SPAGHETTI_END_Y as f32, y))
}

/// Spaghetti depth ramp: `0` at `d < 4`, `1` by `d = 16`.
#[allow(clippy::cast_precision_loss)]
pub(super) fn spaghetti_depth_weight(depth: f32) -> f32 {
    ramp(
        SPAGHETTI_DEPTH_START as f32,
        SPAGHETTI_DEPTH_FULL as f32,
        depth,
    )
}

/// One column of the `y` lattice with a two-cell cache (`F1.3`).
///
/// Consecutive blocks walk the cells in order, so a cell is evaluated exactly once
/// and never at all when the roof test already excluded it.
struct LatticeColumn<'a> {
    /// The volume that owns the noise streams.
    volume: &'a Volume,
    /// World block `x` of the column.
    x: i32,
    /// World block `z` of the column.
    z: i32,
    /// The column's pre-detail `HeightTile` top (`F2.3` weights).
    top: i32,
    /// Cell whose value is cached in `value`.
    cell: i32,
    /// Cached value of `cell`.
    value: f32,
    /// Cell whose value is cached in `next_value`.
    next_cell: i32,
    /// Cached value of `next_cell`.
    next_value: f32,
    /// Taps spent by this column so far.
    taps: u32,
}

impl<'a> LatticeColumn<'a> {
    /// A cache with no cell loaded.
    fn new(volume: &'a Volume, x: i32, z: i32, top: i32) -> Self {
        Self {
            volume,
            x,
            z,
            top,
            cell: i32::MIN,
            value: 0.0,
            next_cell: i32::MIN,
            next_value: 0.0,
            taps: 0,
        }
    }

    /// The density at `cell`, promoting a cached neighbour when it matches.
    fn density(&mut self, cell: i32) -> f32 {
        if cell == self.cell {
            return self.value;
        }
        if cell == self.next_cell {
            self.cell = self.next_cell;
            self.value = self.next_value;
            self.next_cell = i32::MIN;
            return self.value;
        }
        self.cell = cell;
        self.value = self.evaluate(cell);
        self.next_cell = i32::MIN;
        self.value
    }

    /// The density at `cell + 1`, evaluated at most once per cell.
    fn next_density(&mut self) -> f32 {
        let wanted = self.cell.saturating_add(1);
        if self.next_cell != wanted {
            self.next_cell = wanted;
            self.next_value = self.evaluate(wanted);
        }
        self.next_value
    }

    /// One lattice sample: four taps at the cell centre (`F2.1`).
    #[allow(clippy::cast_precision_loss)]
    fn evaluate(&mut self, cell: i32) -> f32 {
        let y = (cell.saturating_mul(MACRO_CELL)) as f32 + 0.5;
        let sample =
            self.volume
                .cave_density(self.x as f32 + 0.5, y, self.z as f32 + 0.5, self.top);
        self.taps = self.taps.saturating_add(u32::from(sample.taps));
        sample.density
    }
}

/// The highest solid block of a column, if it has one.
fn highest_solid(column: &[BlockId; COLUMN_HEIGHT], air: BlockId) -> Option<i32> {
    for y in (1..COLUMN_HEIGHT).rev() {
        if column[y] != air {
            return i32::try_from(y).ok();
        }
    }
    None
}

/// Fixed-order linear interpolation. No `mul_add`.
fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

/// Hermite smoothstep on `[0, 1]`, clamped outside.
fn smooth01(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Hermite smoothstep between two edges, clamped outside.
fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let span = edge1 - edge0;
    if span <= 0.0 {
        return if value < edge0 { 0.0 } else { 1.0 };
    }
    smooth01((value - edge0) / span)
}
