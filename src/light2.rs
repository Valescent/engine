//! `W7` — per-column sky and block light with incremental repair.
//!
//! `L1.1`, `L1.2`, `L2.2`–`L2.4`, `L3.1`–`L3.3`. One job lights a whole
//! 12-section column: twelve 32³ sections stacked to `WORLD_HEIGHT`, plus a
//! one-block border borrowed from the 8 horizontal neighbours. There is no sky
//! summary, no cross-job dependency, and no persistence: light is a pure
//! function of blocks and is recomputed on load.
//!
//! # Storage
//!
//! Light is 15 levels, two channels (sky, block), one nibble per cell.
//! [`ColumnLight`] holds one [`LightSlot`] per section per channel. A slot is
//! either [`LightSlot::Uniform`] (one byte, no allocation — all air is sky 15 /
//! block 0, all solid is sky 0) or [`LightSlot::Packed`] (a [`NibbleArray`] of
//! 2048 words). Promotion and demotion happen on write, so a section that
//! becomes uniform again releases its array.
//!
//! # Propagation
//!
//! Pass 1 seeds the sky vertically from the ceiling. Entering a cell costs its
//! material's `sky_atten`, so open air loses nothing straight down while water
//! loses `WATER_SKY_ATTEN` on every block of the descent. Pass 2 is a lateral BFS
//! seeded only at cells that can improve a neighbour, costing `max(1, sky_atten)`
//! per lateral step and the same zero-cost-down rule when it moves straight down,
//! so light that arrived sideways still runs down a shaft without loss. Sky light
//! never enters an attenuating cell sideways: water takes its sky from the column
//! above, so an open air cell beside a channel cannot flatten the depth charge to
//! the surface level. Block light is a plain BFS from emitters costing
//! `max(1, block_opacity)` per step; water is free for block light, which is what
//! `L2.4` requires.
//!
//! Every loop has a fixed bound. The BFS queue is a fixed-capacity ring buffer
//! ([`QUEUE_CAPACITY`]); if a pathological field ever fills it, the remainder is
//! relaxed by a bounded sweep that reaches the same fixed point.
//!
//! # Repair
//!
//! [`ColumnLight::relight_column`] is two-phase: the writable region is cleared,
//! then re-lit from sky seeds, emitters, and the unchanged cells around it.
//! Every cell is a maximum over sources, so the result is the unique fixed point
//! and cannot depend on order. The writable region is the whole border ring for
//! every `y`, and the interior from the floor up to 15 blocks above the highest
//! touched section. The floor matters because the vertical sky seed is lossless:
//! a block placed at `y` darkens every cell below it, while lateral light cannot
//! reach more than 15 blocks. Everything above the region is unchanged. A fuzz
//! test pins bit-equality with a fresh column.
//!
//! The border ring is recomputed rather than stored. `ColumnLight` keeps only
//! the 12 owned sections, so a bounded repair has to re-derive the ring from
//! blocks; storing it would let the repair touch just the edited halo.

use crate::description::{SECTION_SIZE, SECTIONS_PER_COLUMN, WORLD_HEIGHT};
use crate::padded::{LOCAL_MAX, LOCAL_MIN, PADDED_WIDTH, PaddedLight};
use crate::section::{BlockId, Section};

/// Highest light level (`L1.1`).
pub const LIGHT_LEVELS: u8 = 15;

/// Skylight lost per water block (`L2.4`).
pub const WATER_SKY_ATTEN: u8 = 1;

/// Halo, in blocks, that an edit can influence (`L3.1`).
pub const RELIGHT_RADIUS: i32 = 15;

/// Capacity of the fixed BFS ring buffer.
pub const QUEUE_CAPACITY: usize = 16 * 32_768;

/// Sections per column (`I1.3`).
pub const COLUMN_SECTIONS: usize = 12;

/// Cells in one section.
pub const SECTION_CELLS: usize = 32_768;

/// Height of a whole column in blocks (`C4.3`).
pub const COLUMN_HEIGHT: i32 = 384;

/// `u64` words in one packed section.
pub const NIBBLE_WORDS: usize = 2048;

/// Block id of air, by registry convention.
pub const AIR_ID: BlockId = 0;

/// Block id of leaves in the current registry.
pub const LEAVES_ID: BlockId = 5;

/// Block id of the lamp in the current registry.
pub const LAMP_ID: BlockId = 6;

/// Block id of water in the current registry.
pub const WATER_ID: BlockId = 7;

/// Light emitted by a lamp (`L2.1` is still open, so this is the provisional one).
pub const LAMP_EMISSION: u8 = 14;

/// Provisional id range for cross-quad plants, which never occlude (`H4.2`).
///
/// `A5.1` has not pinned a block registry with a `render_shape` field, so the
/// default table cannot identify plants. This range is a placeholder: replace
/// it with the manifest lookup when the registry lands.
pub const CROSS_QUAD_IDS: core::ops::RangeInclusive<BlockId> = 64..=95;

/// Block returned for `y < 0`: bedrock is opaque, so stone's id stands in.
pub const BEDROCK_ID: BlockId = 1;

const WORLD_TOP: i32 = COLUMN_HEIGHT - 1;

/// [`PADDED_WIDTH`] as the coordinate type.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "PADDED_WIDTH is 34, far inside i32"
)]
const PADDED_AXIS_I: i32 = PADDED_WIDTH as i32;

/// Cells in the padded column: `34 × 34 × 384`.
const PADDED_CELLS: usize = PADDED_WIDTH * PADDED_WIDTH * 384;

/// Nibble words in the padded column.
const PADDED_WORDS: usize = PADDED_CELLS / 16;

/// A section is 32 cells wide on every axis.
const LOCAL_MASK: usize = 31;

const _: () = assert!(SECTIONS_PER_COLUMN == 12);
const _: () = assert!(SECTION_SIZE == 32);
const _: () = assert!(WORLD_HEIGHT == 384);
const _: () = assert!(PADDED_CELLS.is_multiple_of(16));
const _: () = assert!(QUEUE_CAPACITY.is_power_of_two());

/// Horizontal neighbour offsets, in border-slot order.
const BORDER_OFFSETS: [(i32, i32); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];

/// Fixed neighbour order: `-X, +X, -Z, +Z, -Y, +Y` (`L1.1`).
const NEIGHBOURS: [(i32, i32, i32); 6] = [
    (-1, 0, 0),
    (1, 0, 0),
    (0, 0, -1),
    (0, 0, 1),
    (0, -1, 0),
    (0, 1, 0),
];

/// Per-material light behaviour (`L1.1`, `L2.4`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LightProps {
    /// Skylight lost on entering the block.
    pub sky_atten: u8,
    /// Extra block-light loss on entering the block.
    pub block_opacity: u8,
}

impl LightProps {
    /// Fully transparent: air, cross-quads.
    pub const CLEAR: Self = Self {
        sky_atten: 0,
        block_opacity: 0,
    };

    /// Blocks both channels.
    pub const OPAQUE: Self = Self {
        sky_atten: LIGHT_LEVELS,
        block_opacity: LIGHT_LEVELS,
    };

    /// Attenuates skylight by [`WATER_SKY_ATTEN`] and passes block light.
    pub const WATER: Self = Self {
        sky_atten: WATER_SKY_ATTEN,
        block_opacity: 0,
    };

    /// Passes skylight and dims block light.
    pub const LEAVES: Self = Self {
        sky_atten: 0,
        block_opacity: 1,
    };

    /// Builds a rule, masking both fields to four bits.
    #[must_use]
    pub const fn new(sky_atten: u8, block_opacity: u8) -> Self {
        Self {
            sky_atten: sky_atten & 0x0F,
            block_opacity: block_opacity & 0x0F,
        }
    }
}

/// A full block-id → [`LightProps`] table.
///
/// A plain array, never a map, so a lookup in a BFS step is an index. Ids at or
/// above 256 use the fallback. The fallback defaults to [`LightProps::OPAQUE`],
/// so an unknown material can only ever under-light, never leak.
#[derive(Clone, Debug)]
pub struct LightTable {
    props: [LightProps; 256],
    emission: [u8; 256],
    fallback: LightProps,
}

impl Default for LightTable {
    fn default() -> Self {
        default_table()
    }
}

/// The v1 default material table (`L1.1`, `L2.4`).
///
/// Air and the provisional cross-quad range are `0/0`, leaves are `0/1`,
/// water is `1/0`, and everything else is opaque `15/15`. A lamp is opaque to
/// skylight, passes block light, and emits [`LAMP_EMISSION`].
#[must_use]
pub fn default_table() -> LightTable {
    let mut table = LightTable::new(LightProps::OPAQUE);
    table.set(AIR_ID, LightProps::CLEAR);
    table.set(LEAVES_ID, LightProps::LEAVES);
    table.set(WATER_ID, LightProps::WATER);
    table.set(LAMP_ID, LightProps::new(LIGHT_LEVELS, 0));
    table.set_emission(LAMP_ID, LAMP_EMISSION);
    for id in CROSS_QUAD_IDS {
        table.set(id, LightProps::CLEAR);
    }
    table
}

impl LightTable {
    /// A table where every id maps to `fallback`.
    #[must_use]
    pub fn new(fallback: LightProps) -> Self {
        Self {
            props: [fallback; 256],
            emission: [0; 256],
            fallback,
        }
    }

    /// Sets the rule for one id. Ids at or above 256 are not table entries.
    pub fn set(&mut self, id: BlockId, props: LightProps) {
        if let Some(slot) = self.props.get_mut(usize::from(id)) {
            *slot = props;
        }
    }

    /// Sets how much light one id emits. Ids at or above 256 never emit.
    pub fn set_emission(&mut self, id: BlockId, level: u8) {
        if let Some(slot) = self.emission.get_mut(usize::from(id)) {
            *slot = level & 0x0F;
        }
    }

    /// The rule for one id.
    #[must_use]
    pub fn props(&self, id: BlockId) -> LightProps {
        self.props
            .get(usize::from(id))
            .copied()
            .unwrap_or(self.fallback)
    }

    /// The emission for one id.
    #[must_use]
    pub fn emission(&self, id: BlockId) -> u8 {
        self.emission.get(usize::from(id)).copied().unwrap_or(0)
    }

    /// The rule for ids at or above the table size.
    #[must_use]
    pub fn fallback(&self) -> LightProps {
        self.fallback
    }
}

/// Four-bit light values for one 32³ section, 16 nibbles per `u64`.
///
/// The representation is exactly [`NIBBLE_WORDS`] words = 16384 bytes = 32768
/// nibbles. A 16-entry histogram is kept alongside so [`Self::is_uniform`] and
/// write-time demotion are constant time; without it a single write would have
/// to rescan the section.
#[derive(Clone)]
pub struct NibbleArray {
    data: Box<[u64]>,
    counts: [u16; 16],
}

impl NibbleArray {
    /// A packed array whose cells all hold `value`.
    #[must_use]
    pub fn new(value: u8) -> Self {
        let value = value & 0x0F;
        let mut counts = [0_u16; 16];
        counts[usize::from(value)] = section_cells_u16();
        Self {
            data: vec![splat(value); NIBBLE_WORDS].into_boxed_slice(),
            counts,
        }
    }

    /// Reads one cell. Indices are masked to the section volume.
    #[must_use]
    pub fn get(&self, index: usize) -> u8 {
        let index = index & (SECTION_CELLS - 1);
        nibble(self.data[index >> 4] >> ((index & 15) << 2))
    }

    /// Writes one cell, masking `value` to four bits. Indices are masked.
    pub fn set(&mut self, index: usize, value: u8) {
        let index = index & (SECTION_CELLS - 1);
        let value = value & 0x0F;
        let shift = (index & 15) << 2;
        let word = &mut self.data[index >> 4];
        let previous = nibble(*word >> shift);
        if previous == value {
            return;
        }
        *word = (*word & !(0x0F_u64 << shift)) | (u64::from(value) << shift);
        self.counts[usize::from(previous)] -= 1;
        self.counts[usize::from(value)] += 1;
    }

    /// Sets every cell to `value`.
    pub fn set_all(&mut self, value: u8) {
        let value = value & 0x0F;
        self.data.fill(splat(value));
        self.counts = [0; 16];
        self.counts[usize::from(value)] = section_cells_u16();
    }

    /// The single value every cell holds, or `None` when the section varies.
    #[must_use]
    pub fn is_uniform(&self) -> Option<u8> {
        let mut found = None;
        for (value, count) in self.counts.iter().enumerate() {
            if *count == 0 {
                continue;
            }
            if found.is_some() {
                return None;
            }
            found = Some(u8::try_from(value).expect("a nibble fits u8"));
        }
        found
    }

    /// Words in the array, for the metrics and save layers.
    #[must_use]
    pub fn words(&self) -> &[u64] {
        &self.data
    }
}

#[allow(
    clippy::missing_fields_in_debug,
    reason = "the packed words are summarised by length and uniformity"
)]
impl std::fmt::Debug for NibbleArray {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NibbleArray")
            .field("words", &self.data.len())
            .field("uniform", &self.is_uniform())
            .finish()
    }
}

/// One section's light, either uniform or packed (`K1.5`).
#[derive(Clone)]
pub enum LightSlot {
    /// One value for the whole section; no allocation.
    Uniform(u8),
    /// Packed nibbles.
    Packed(NibbleArray),
}

impl LightSlot {
    /// A uniform slot.
    #[must_use]
    pub const fn uniform(value: u8) -> Self {
        Self::Uniform(value & 0x0F)
    }

    /// Reads one cell.
    #[must_use]
    pub fn get(&self, index: usize) -> u8 {
        match self {
            Self::Uniform(value) => *value & 0x0F,
            Self::Packed(cells) => cells.get(index),
        }
    }

    /// Writes one cell, promoting a uniform slot and demoting a packed one that
    /// becomes uniform again.
    pub fn set(&mut self, index: usize, value: u8) {
        let value = value & 0x0F;
        match self {
            Self::Uniform(current) => {
                if *current & 0x0F == value {
                    return;
                }
                let mut cells = NibbleArray::new(*current);
                cells.set(index, value);
                *self = Self::Packed(cells);
            }
            Self::Packed(cells) => {
                if cells.get(index) == value {
                    return;
                }
                cells.set(index, value);
                if let Some(uniform) = cells.is_uniform() {
                    *self = Self::Uniform(uniform);
                }
            }
        }
    }

    /// Sets every cell, releasing any allocation.
    pub fn set_all(&mut self, value: u8) {
        *self = Self::uniform(value);
    }

    /// The single value the slot holds, or `None` when it varies.
    #[must_use]
    pub fn is_uniform(&self) -> Option<u8> {
        match self {
            Self::Uniform(value) => Some(*value & 0x0F),
            Self::Packed(cells) => cells.is_uniform(),
        }
    }

    /// Whether the slot owns a packed array.
    #[must_use]
    pub fn is_packed(&self) -> bool {
        matches!(self, Self::Packed(_))
    }
}

impl LightSlot {
    /// Whether two slots hold the same values, regardless of representation.
    fn differs(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Uniform(a), Self::Uniform(b)) => a != b,
            (Self::Uniform(value), Self::Packed(cells))
            | (Self::Packed(cells), Self::Uniform(value)) => {
                cells.is_uniform() != Some(*value & 0x0F)
            }
            (Self::Packed(a), Self::Packed(b)) => a.words() != b.words(),
        }
    }
}

impl std::fmt::Debug for LightSlot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Uniform(value) => formatter.debug_tuple("Uniform").field(value).finish(),
            Self::Packed(cells) => formatter.debug_tuple("Packed").field(cells).finish(),
        }
    }
}

/// Sky and block light for one 12-section column (`L1.1`).
///
/// A fresh column is open air: sky 15 everywhere, block 0, every slot uniform
/// and allocation-free.
#[derive(Clone)]
pub struct ColumnLight {
    sky: [LightSlot; COLUMN_SECTIONS],
    block: [LightSlot; COLUMN_SECTIONS],
}

impl Default for ColumnLight {
    fn default() -> Self {
        Self::new()
    }
}

impl ColumnLight {
    /// An open-air column.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sky: std::array::from_fn(|_| LightSlot::Uniform(LIGHT_LEVELS)),
            block: std::array::from_fn(|_| LightSlot::Uniform(0)),
        }
    }

    /// Skylight at a cell. `x` and `z` wrap to `0..32`; outside the world,
    /// above is 15 and below is 0 (`C4.4`).
    #[must_use]
    pub fn sky(&self, x: i32, y: i32, z: i32) -> u8 {
        sample(&self.sky, x, y, z, LIGHT_LEVELS)
    }

    /// Block light at a cell, with the same coordinate rules as [`Self::sky`].
    #[must_use]
    pub fn block(&self, x: i32, y: i32, z: i32) -> u8 {
        sample(&self.block, x, y, z, 0)
    }

    /// The sky slot of one section.
    #[must_use]
    pub fn sky_slot(&self, section: usize) -> &LightSlot {
        &self.sky[section % COLUMN_SECTIONS]
    }

    /// The block slot of one section.
    #[must_use]
    pub fn block_slot(&self, section: usize) -> &LightSlot {
        &self.block[section % COLUMN_SECTIONS]
    }

    /// Whether one section's sky or block light differs from another column.
    ///
    /// Representation is ignored: a uniform slot equals a packed slot holding
    /// the same values. Used to remesh only the sections an edit changed.
    #[must_use]
    pub fn section_differs(&self, other: &Self, section: usize) -> bool {
        let index = section % COLUMN_SECTIONS;
        self.sky[index].differs(&other.sky[index]) || self.block[index].differs(&other.block[index])
    }

    /// Whether every slot in both channels is uniform.
    #[must_use]
    pub fn is_uniform(&self) -> bool {
        self.sky
            .iter()
            .chain(self.block.iter())
            .all(|slot| slot.is_uniform().is_some())
    }

    /// Bytes held by packed slots, for memory accounting.
    #[must_use]
    pub fn packed_bytes(&self) -> usize {
        self.sky
            .iter()
            .chain(self.block.iter())
            .filter(|slot| slot.is_packed())
            .count()
            * (NIBBLE_WORDS * size_of::<u64>())
    }

    /// Copies one section's light into the shared padded scratch for the mesher.
    ///
    /// The `32³` interior is this column's light for `section`. The vertical pad
    /// reads the adjacent section of the same column, so a cell on a section
    /// boundary sees the light it actually has; `y < 0` is dark and
    /// `y >= WORLD_HEIGHT` is full sky (`L1.1`). The horizontal pad mirrors the
    /// edge row, the `I2.3` world-edge rule, because a finished [`ColumnLight`]
    /// does not retain its neighbours. A caller that has them overlays the
    /// border with [`PaddedLight::set`].
    #[must_use]
    pub fn padded_section(&self, section: usize) -> PaddedLight {
        let section = section % COLUMN_SECTIONS;
        let mut padded = PaddedLight::default();
        for y in LOCAL_MIN..=LOCAL_MAX {
            for z in LOCAL_MIN..=LOCAL_MAX {
                for x in LOCAL_MIN..=LOCAL_MAX {
                    let (sky, block) = self.padded_sample(section, x, y, z);
                    padded.set(x, y, z, sky, block);
                }
            }
        }
        padded
    }

    /// One padded light cell for [`Self::padded_section`].
    fn padded_sample(&self, section: usize, x: i32, y: i32, z: i32) -> (u8, u8) {
        let base = i32::try_from(section).expect("a section index fits i32") * 32;
        let world_y = base + y;
        if world_y < 0 {
            return (0, 0);
        }
        let world_y = u32::try_from(world_y).expect("a padded world y is non-negative");
        if world_y >= WORLD_HEIGHT {
            return (LIGHT_LEVELS, 0);
        }
        let slot = usize::try_from(world_y / SECTION_SIZE).expect("a section index fits usize");
        let local_y = usize::try_from(world_y % SECTION_SIZE).expect("a local y fits usize");
        let edge = i32::try_from(SECTION_SIZE).expect("SECTION_SIZE is 32") - 1;
        let local_x = u32::try_from(x.clamp(0, edge)).expect("a mirrored x is non-negative");
        let local_z = u32::try_from(z.clamp(0, edge)).expect("a mirrored z is non-negative");
        let cell = section_cell(
            usize::try_from(local_x).expect("a local x fits usize"),
            local_y,
            usize::try_from(local_z).expect("a local z fits usize"),
        );
        (self.sky[slot].get(cell), self.block[slot].get(cell))
    }

    /// Repairs light after a batch of edits, returning how many cells changed.
    ///
    /// The caller has already written every edit into `blocks`; `edits` says
    /// which cells changed so the repair can bound its region (`L3.1`). `self`
    /// must already hold the light for `blocks` before the batch, which is what
    /// [`compute_column`] produces. The two-phase clear/re-light below produces
    /// the same fixed point as [`compute_column`], which is the `L3.3` gate.
    ///
    /// `props` is the material table: a single [`LightProps`] record cannot
    /// describe a whole column, so the parameter is the table type.
    pub fn relight_column(
        &mut self,
        blocks: &ColumnBlocks,
        props: &LightTable,
        edits: &[Edit],
    ) -> u32 {
        let Some((y_lo, y_hi)) = relight_bounds(edits) else {
            return 0;
        };
        self.relight_span(blocks, props, y_lo, y_hi)
    }

    /// Repairs light for an explicit vertical band.
    ///
    /// The caller guarantees no cell outside `y_lo..=y_hi` changed. The band is
    /// widened by one cell so the flood has unchanged values to stop against.
    /// [`Self::relight_column`] derives the band from edits; a column whose own
    /// sky seed did not move can use a narrow band around the edit instead of
    /// repairing from `y = 0`.
    pub fn relight_span(
        &mut self,
        blocks: &ColumnBlocks,
        props: &LightTable,
        y_lo: i32,
        y_hi: i32,
    ) -> u32 {
        self.relight_span_view(&blocks.view(), props, y_lo, y_hi)
    }

    /// [`Self::relight_span`] over a borrowed column, with no copy.
    pub fn relight_span_view(
        &mut self,
        view: &ColumnView<'_>,
        props: &LightTable,
        y_lo: i32,
        y_hi: i32,
    ) -> u32 {
        let lo = (y_lo - 1).max(0);
        let hi = (y_hi + 1).min(WORLD_TOP);
        if hi < lo {
            return 0;
        }
        let mut scratch = Scratch::new();
        scratch.set_region(lo, hi);
        scratch.build(view, props);
        scratch.copy_interior_from(self);
        scratch.prefill(Channel::Sky, LIGHT_LEVELS);
        scratch.prefill(Channel::Block, 0);
        scratch.vertical_sky(view.top_span());
        scratch.seed_sky();
        scratch.flood(Channel::Sky);
        scratch.seed_block();
        scratch.flood(Channel::Block);
        scratch.store(self)
    }
}

impl PartialEq for ColumnLight {
    /// Cell-wise equality. A uniform slot equals a packed slot holding the same
    /// value, because light is a function, not a representation.
    fn eq(&self, other: &Self) -> bool {
        self.sky
            .iter()
            .zip(other.sky.iter())
            .all(|(a, b)| (0..SECTION_CELLS).all(|index| a.get(index) == b.get(index)))
            && self
                .block
                .iter()
                .zip(other.block.iter())
                .all(|(a, b)| (0..SECTION_CELLS).all(|index| a.get(index) == b.get(index)))
    }
}

impl Eq for ColumnLight {}

#[allow(
    clippy::missing_fields_in_debug,
    reason = "the two slot arrays are summarised by uniformity and footprint"
)]
impl std::fmt::Debug for ColumnLight {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ColumnLight")
            .field("uniform", &self.is_uniform())
            .field("packed_bytes", &self.packed_bytes())
            .finish()
    }
}

/// The blocks one column job reads: 12 owned sections plus 8 border columns.
///
/// A border read is copied once, here, so a job never holds a live neighbour
/// borrow (`I2.3`). Missing borders mirror the adjacent interior edge, which is
/// what a resolved world edge does; light is only built when the column and all
/// neighbours are resolved, so `None` is a degenerate case.
#[derive(Clone, Debug)]
pub struct ColumnBlocks {
    sections: [Section; COLUMN_SECTIONS],
    borders: [Option<[Section; COLUMN_SECTIONS]>; 8],
    top_span: Option<i32>,
}

/// A borrowed column and its eight border columns.
///
/// This is the read-only view the repair path uses to avoid copying a whole
/// column (and every border) before relighting.
#[derive(Clone, Copy, Debug)]
pub struct ColumnView<'a> {
    sections: &'a [Section; COLUMN_SECTIONS],
    borders: [Option<&'a [Section; COLUMN_SECTIONS]>; 8],
    top_span: Option<i32>,
}

impl<'a> ColumnView<'a> {
    /// Borrows a column and its border order `(-1,-1) ..= (1,1)`.
    #[must_use]
    pub fn new(
        sections: &'a [Section; COLUMN_SECTIONS],
        borders: [Option<&'a [Section; COLUMN_SECTIONS]>; 8],
    ) -> Self {
        Self {
            sections,
            borders,
            top_span: None,
        }
    }

    /// Caches the highest `y` whose block is not sky-clear (`L1.1`).
    #[must_use]
    pub fn with_top_span(mut self, top: i32) -> Self {
        self.top_span = Some(top);
        self
    }

    /// The cached span, if any.
    #[must_use]
    pub fn top_span(&self) -> Option<i32> {
        self.top_span
    }

    /// One section of the column.
    #[must_use]
    pub fn section(&self, index: usize) -> &Section {
        &self.sections[index % COLUMN_SECTIONS]
    }

    /// Reads a block in `x, z ∈ -1..=32` and `y ∈ 0..384` (`I2.3`).
    #[must_use]
    pub fn get(&self, x: i32, y: i32, z: i32) -> BlockId {
        let Some(y) = u32::try_from(y).ok().filter(|value| *value < WORLD_HEIGHT) else {
            return if y < 0 { BEDROCK_ID } else { AIR_ID };
        };
        let section = usize::try_from(y / SECTION_SIZE).expect("a section index fits usize");
        let local_y = y % SECTION_SIZE;
        let dx = edge(x);
        let dz = edge(z);
        if dx == 0 && dz == 0 {
            let local_x = u32::try_from(x).expect("an interior x is non-negative");
            let local_z = u32::try_from(z).expect("an interior z is non-negative");
            return self.sections[section].get(local_x, local_y, local_z);
        }
        let local_x = neighbour_local(x, dx);
        let local_z = neighbour_local(z, dz);
        if let Some(column) = self.borders[border_slot(dx, dz)] {
            return column[section].get(local_x, local_y, local_z);
        }
        self.sections[section].get(
            u32::try_from(mirror(x, dx)).expect("a mirrored x is non-negative"),
            local_y,
            u32::try_from(mirror(z, dz)).expect("a mirrored z is non-negative"),
        )
    }
}

impl ColumnBlocks {
    /// A borrowed view of this owned column.
    #[must_use]
    pub fn view(&self) -> ColumnView<'_> {
        ColumnView {
            sections: &self.sections,
            borders: std::array::from_fn(|slot| self.borders[slot].as_ref()),
            top_span: self.top_span,
        }
    }

    /// Copies the column and the 8 horizontal neighbour columns.
    ///
    /// Border order is `(-1,-1), (0,-1), (1,-1), (-1,0), (1,0), (-1,1), (0,1),
    /// (1,1)` in `(dx, dz)`. A border is a whole column because the job reads
    /// every `y`, not one section.
    #[must_use]
    pub fn new(
        sections: [Section; COLUMN_SECTIONS],
        borders: &[Option<&[Section; COLUMN_SECTIONS]>; 8],
    ) -> Self {
        Self {
            sections,
            borders: std::array::from_fn(|slot| borders[slot].cloned()),
            top_span: None,
        }
    }

    /// Caches the highest `y` whose block is not sky-clear, so pass 1 above it
    /// is `O(1)` (`L1.1`).
    ///
    /// For a dry column this is `top_solid_y`. For an ocean column it must be
    /// the highest water block, because water attenuates: the hint promises
    /// "every cell above this `y` is air", and a wrong hint changes light.
    pub fn set_top_span(&mut self, top: i32) {
        self.top_span = Some(top);
    }

    /// The cached span, if any.
    #[must_use]
    pub fn top_span(&self) -> Option<i32> {
        self.top_span
    }

    /// One owned section.
    #[must_use]
    pub fn section(&self, index: usize) -> &Section {
        &self.sections[index % COLUMN_SECTIONS]
    }

    /// Reads a block in `x, z ∈ -1..=32` and `y ∈ 0..384` (`I2.3`).
    ///
    /// `y < 0` is bedrock and `y >= WORLD_HEIGHT` is air. Zero and 32 address
    /// the border columns; the corners read the diagonal border.
    #[must_use]
    pub fn get(&self, x: i32, y: i32, z: i32) -> BlockId {
        let Some(y) = u32::try_from(y).ok().filter(|value| *value < WORLD_HEIGHT) else {
            return if y < 0 { BEDROCK_ID } else { AIR_ID };
        };
        let section = usize::try_from(y / SECTION_SIZE).expect("a section index fits usize");
        let local_y = y % SECTION_SIZE;
        let dx = edge(x);
        let dz = edge(z);
        if dx == 0 && dz == 0 {
            let local_x = u32::try_from(x).expect("an interior x is non-negative");
            let local_z = u32::try_from(z).expect("an interior z is non-negative");
            return self.sections[section].get(local_x, local_y, local_z);
        }
        let local_x = neighbour_local(x, dx);
        let local_z = neighbour_local(z, dz);
        if let Some(column) = &self.borders[border_slot(dx, dz)] {
            return column[section].get(local_x, local_y, local_z);
        }
        self.sections[section].get(
            u32::try_from(mirror(x, dx)).expect("a mirrored x is non-negative"),
            local_y,
            u32::try_from(mirror(z, dz)).expect("a mirrored z is non-negative"),
        )
    }

    /// Writes one interior block.
    pub fn set(&mut self, x: i32, y: i32, z: i32, block: BlockId) {
        assert!(
            (0..32).contains(&x) && (0..32).contains(&z) && (0..COLUMN_HEIGHT).contains(&y),
            "only interior cells can be edited"
        );
        let section = usize::try_from(y / 32).expect("a section index fits usize");
        self.sections[section].set(
            u32::try_from(x).expect("an interior x is non-negative"),
            u32::try_from(y % 32).expect("a local y is non-negative"),
            u32::try_from(z).expect("an interior z is non-negative"),
            block,
        );
    }

    /// Sections touched by `edits` plus the 15-block halo (`L3.1`).
    ///
    /// Every section at or below the halo top is included: the lossless
    /// vertical sky rule lets an edit darken the whole column beneath it.
    #[must_use]
    pub fn affected_sections(edits: &[Edit]) -> [bool; COLUMN_SECTIONS] {
        let mut touched = [false; COLUMN_SECTIONS];
        if let Some((low, high)) = relight_bounds(edits) {
            for (section, flag) in touched.iter_mut().enumerate() {
                let top = i32::try_from(section * 32).expect("a section top fits i32");
                *flag = top + 31 >= low && top <= high;
            }
        }
        touched
    }
}

/// One changed cell. The blocks are already updated when a repair runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Edit {
    /// Local `x` in `0..32`.
    pub x: i32,
    /// World `y` in `0..384`.
    pub y: i32,
    /// Local `z` in `0..32`.
    pub z: i32,
    /// The block now in the cell.
    pub block: BlockId,
}

impl Edit {
    /// Builds an edit record.
    #[must_use]
    pub const fn new(x: i32, y: i32, z: i32, block: BlockId) -> Self {
        Self { x, y, z, block }
    }
}

/// Counters for one job, for the top-span test and the metrics layer.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LightStats {
    /// Cells visited by the vertical sky pass.
    pub vertical_cells: usize,
    /// Cells pushed as sky BFS seeds.
    pub sky_seeds: usize,
    /// Sky BFS pops.
    pub sky_steps: usize,
    /// Emitters pushed as block-light seeds.
    pub block_seeds: usize,
    /// Block BFS pops.
    pub block_steps: usize,
    /// Times the fixed ring buffer filled and the bounded sweep took over.
    pub queue_overflows: usize,
}

/// Lights one whole column from blocks (`L1.2`).
#[must_use]
pub fn compute_column(blocks: &ColumnBlocks, props: &LightTable) -> ColumnLight {
    compute_column_stats(blocks, props).0
}

/// [`compute_column`], also returning the job counters.
#[must_use]
pub fn compute_column_stats(
    blocks: &ColumnBlocks,
    props: &LightTable,
) -> (ColumnLight, LightStats) {
    let mut scratch = Scratch::new();
    scratch.set_region(0, WORLD_TOP);
    scratch.build(&blocks.view(), props);
    scratch.prefill(Channel::Sky, LIGHT_LEVELS);
    scratch.prefill(Channel::Block, 0);
    scratch.vertical_sky(blocks.top_span);
    scratch.seed_sky();
    scratch.flood(Channel::Sky);
    scratch.seed_block();
    scratch.flood(Channel::Block);
    (scratch.extract(), scratch.stats)
}

/// A channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Channel {
    Sky,
    Block,
}

/// Fixed-capacity ring of packed BFS entries.
///
/// An entry packs `px | pz << 6 | y << 12 | level << 21`, so a pop needs no
/// division and the whole queue is 2 MiB of `u32`.
struct Ring {
    data: Box<[u32]>,
    head: usize,
    len: usize,
}

impl Ring {
    fn new() -> Self {
        Self {
            data: vec![0_u32; QUEUE_CAPACITY].into_boxed_slice(),
            head: 0,
            len: 0,
        }
    }

    fn push(&mut self, entry: u32) -> bool {
        if self.len == self.data.len() {
            return false;
        }
        let slot = (self.head + self.len) & (QUEUE_CAPACITY - 1);
        self.data[slot] = entry;
        self.len += 1;
        true
    }

    fn pop(&mut self) -> Option<u32> {
        if self.len == 0 {
            return None;
        }
        let entry = self.data[self.head];
        self.head = (self.head + 1) & (QUEUE_CAPACITY - 1);
        self.len -= 1;
        Some(entry)
    }
}

/// Per-job scratch: material bits, two light planes, and the queue.
struct Scratch {
    cells: Box<[u16]>,
    sky: Box<[u64]>,
    block: Box<[u64]>,
    queue: Ring,
    overflow: bool,
    y_lo: i32,
    y_hi: i32,
    full: bool,
    stats: LightStats,
}

impl Scratch {
    fn new() -> Self {
        Self {
            cells: vec![0_u16; PADDED_CELLS].into_boxed_slice(),
            sky: vec![0_u64; PADDED_WORDS].into_boxed_slice(),
            block: vec![0_u64; PADDED_WORDS].into_boxed_slice(),
            queue: Ring::new(),
            overflow: false,
            y_lo: 0,
            y_hi: WORLD_TOP,
            full: true,
            stats: LightStats::default(),
        }
    }

    fn set_region(&mut self, y_lo: i32, y_hi: i32) {
        self.y_lo = y_lo.max(0);
        self.y_hi = y_hi.min(WORLD_TOP);
        self.full = self.y_lo == 0 && self.y_hi == WORLD_TOP;
    }

    fn light(&self, channel: Channel, index: usize) -> u8 {
        let data = match channel {
            Channel::Sky => &self.sky,
            Channel::Block => &self.block,
        };
        nibble(data[index >> 4] >> ((index & 15) << 2))
    }

    fn write(&mut self, channel: Channel, index: usize, value: u8) {
        let value = u64::from(value & 0x0F);
        let data = match channel {
            Channel::Sky => &mut self.sky,
            Channel::Block => &mut self.block,
        };
        let shift = (index & 15) << 2;
        let word = &mut data[index >> 4];
        *word = (*word & !(0x0F_u64 << shift)) | (value << shift);
    }

    fn in_w(&self, px: usize, pz: usize, y: i32) -> bool {
        is_ring(px, pz) || (y >= self.y_lo && y <= self.y_hi)
    }

    /// Fills the writable region with one value.
    fn prefill(&mut self, channel: Channel, value: u8) {
        if self.full {
            let data = match channel {
                Channel::Sky => &mut self.sky,
                Channel::Block => &mut self.block,
            };
            data.fill(splat(value & 0x0F));
            return;
        }
        for y in self.y_lo..=self.y_hi {
            let py = padded(y);
            for pz in 0..=LOCAL_MASK {
                for px in 0..=LOCAL_MASK {
                    self.write(channel, padded_cell(px + 1, pz + 1, py), value);
                }
            }
        }
        for y in 0..COLUMN_HEIGHT {
            let py = padded(y);
            for px in 0..PADDED_WIDTH {
                self.write(channel, padded_cell(px, 0, py), value);
                self.write(channel, padded_cell(px, PADDED_WIDTH - 1, py), value);
            }
            for pz in 1..PADDED_WIDTH - 1 {
                self.write(channel, padded_cell(0, pz, py), value);
                self.write(channel, padded_cell(PADDED_WIDTH - 1, pz, py), value);
            }
        }
    }

    /// Copies the unchanged light the repair can read into the planes.
    ///
    /// Only two kinds of cell matter: the rows directly above and below the
    /// writable band, and the interior edge lines outside it, which seed the
    /// flood along section walls. The prefill overwrites the band itself, and
    /// the flood only visits writable cells, so nothing else is read.
    fn copy_interior_from(&mut self, light: &ColumnLight) {
        if self.full {
            return;
        }
        for y in [self.y_lo - 1, self.y_hi + 1] {
            if (0..COLUMN_HEIGHT).contains(&y) {
                self.copy_light_row(light, y);
            }
        }
        for y in 0..COLUMN_HEIGHT {
            if (y >= self.y_lo && y <= self.y_hi) || y == self.y_lo - 1 || y == self.y_hi + 1 {
                continue;
            }
            self.copy_light_edges(light, y);
        }
    }

    /// Copies one whole interior row from the existing light.
    fn copy_light_row(&mut self, light: &ColumnLight, y: i32) {
        let py = padded(y);
        for pz in 0..=LOCAL_MASK {
            for px in 0..=LOCAL_MASK {
                let index = padded_cell(px + 1, pz + 1, py);
                let (x, z) = (coord(px), coord(pz));
                self.write(Channel::Sky, index, light.sky(x, y, z));
                self.write(Channel::Block, index, light.block(x, y, z));
            }
        }
    }

    /// Copies the four interior edge lines of one row.
    fn copy_light_edges(&mut self, light: &ColumnLight, y: i32) {
        let py = padded(y);
        for step in 0..=LOCAL_MASK {
            for (px, pz) in [(0, step), (LOCAL_MASK, step), (step, 0), (step, LOCAL_MASK)] {
                let index = padded_cell(px + 1, pz + 1, py);
                let (x, z) = (coord(px), coord(pz));
                self.write(Channel::Sky, index, light.sky(x, y, z));
                self.write(Channel::Block, index, light.block(x, y, z));
            }
        }
    }

    /// Decodes the column and the 8 border columns into material bits.
    fn build(&mut self, view: &ColumnView<'_>, props: &LightTable) {
        let mut decoded: Box<[BlockId; SECTION_CELLS]> = vec![0; SECTION_CELLS]
            .into_boxed_slice()
            .try_into()
            .expect("the scratch matches the section volume");
        for section in 0..COLUMN_SECTIONS {
            let base = section * 32;
            let start = usize::try_from(self.y_lo.max(0)).unwrap_or(0).max(base);
            let end = usize::try_from(self.y_hi.max(0))
                .unwrap_or(0)
                .min(base + LOCAL_MASK);
            if end < start {
                continue;
            }
            view.section(section).write_blocks(&mut decoded);
            for y in start..=end {
                let local = y - base;
                for z in 0..=LOCAL_MASK {
                    for x in 0..=LOCAL_MASK {
                        let id = decoded[section_cell(x, local, z)];
                        self.cells[padded_cell(x + 1, z + 1, y)] = packed_props(props, id);
                    }
                }
            }
        }
        for py in 0..384_usize {
            let y = i32::try_from(py).expect("a padded y fits i32");
            for pz in 0..PADDED_WIDTH {
                for px in 0..PADDED_WIDTH {
                    if !is_ring(px, pz) {
                        continue;
                    }
                    let id = view.get(coord(px), y, coord(pz));
                    self.cells[padded_cell(px, pz, py)] = packed_props(props, id);
                }
            }
        }
    }

    /// Pass 1: sky 15 down from the ceiling, losing only the entering material's
    /// attenuation. Above a cached span the prefill already holds 15.
    fn vertical_sky(&mut self, top_span: Option<i32>) {
        for pz in 0..PADDED_WIDTH {
            for px in 0..PADDED_WIDTH {
                let ring = is_ring(px, pz);
                let (bottom, top) = if ring {
                    (0, WORLD_TOP)
                } else {
                    let top = top_span.map_or(self.y_hi, |span| span.clamp(self.y_lo, self.y_hi));
                    (self.y_lo, top)
                };
                let mut level = if top == WORLD_TOP {
                    LIGHT_LEVELS
                } else {
                    let above = usize::try_from(top + 1).expect("a padded y is non-negative");
                    self.light(Channel::Sky, padded_cell(px, pz, above))
                };
                for y in (bottom..=top).rev() {
                    let py = usize::try_from(y).expect("a padded y is non-negative");
                    let index = padded_cell(px, pz, py);
                    level = level.saturating_sub(sky_atten(self.cells[index]));
                    self.write(Channel::Sky, index, level);
                    self.stats.vertical_cells += 1;
                }
            }
        }
    }

    /// Seeds the sky BFS at cells that can improve a writable neighbour.
    fn seed_sky(&mut self) {
        for y in self.y_lo..=self.y_hi {
            for pz in 0..=LOCAL_MASK {
                for px in 0..=LOCAL_MASK {
                    self.seed_sky_cell(px + 1, pz + 1, y);
                }
            }
        }
        for y in 0..COLUMN_HEIGHT {
            for px in 0..PADDED_WIDTH {
                self.seed_sky_cell(px, 0, y);
                self.seed_sky_cell(px, PADDED_WIDTH - 1, y);
            }
            for pz in 1..PADDED_WIDTH - 1 {
                self.seed_sky_cell(0, pz, y);
                self.seed_sky_cell(PADDED_WIDTH - 1, pz, y);
            }
        }
        self.seed_outer(Channel::Sky);
    }

    fn seed_sky_cell(&mut self, px: usize, pz: usize, y: i32) {
        let index = padded_cell(px, pz, padded(y));
        let level = self.light(Channel::Sky, index);
        if level > 1 && self.improves(Channel::Sky, px, pz, y, level) {
            self.push(px, pz, y, level);
            self.stats.sky_seeds += 1;
        }
    }

    /// Seeds block light from emitters, then from the unchanged cells around the
    /// writable region.
    fn seed_block(&mut self) {
        for y in self.y_lo..=self.y_hi {
            for pz in 0..=LOCAL_MASK {
                for px in 0..=LOCAL_MASK {
                    self.seed_block_cell(px + 1, pz + 1, y);
                }
            }
        }
        for y in 0..COLUMN_HEIGHT {
            for px in 0..PADDED_WIDTH {
                self.seed_block_cell(px, 0, y);
                self.seed_block_cell(px, PADDED_WIDTH - 1, y);
            }
            for pz in 1..PADDED_WIDTH - 1 {
                self.seed_block_cell(0, pz, y);
                self.seed_block_cell(PADDED_WIDTH - 1, pz, y);
            }
        }
        self.seed_outer(Channel::Block);
    }

    fn seed_block_cell(&mut self, px: usize, pz: usize, y: i32) {
        let index = padded_cell(px, pz, padded(y));
        let emission = emission(self.cells[index]);
        if emission > 0 {
            self.write(Channel::Block, index, emission);
            self.push(px, pz, y, emission);
            self.stats.block_seeds += 1;
        }
    }

    /// Seeds from cells outside the writable region that can light into it.
    fn seed_outer(&mut self, channel: Channel) {
        if self.full {
            return;
        }
        let bands = [self.y_lo - 1, self.y_hi + 1];
        for y in bands {
            let Ok(py) = usize::try_from(y) else {
                continue;
            };
            if py >= 384 {
                continue;
            }
            for pz in 0..=LOCAL_MASK {
                for px in 0..=LOCAL_MASK {
                    self.seed_cell(channel, px + 1, pz + 1, py);
                }
            }
        }
        // The edge lines of the interior are adjacent to the border ring for
        // every y, including y outside the halo.
        for py in 0..384_usize {
            let y = i32::try_from(py).expect("a padded y fits i32");
            if y >= self.y_lo && y <= self.y_hi {
                continue;
            }
            for step in 0..=LOCAL_MASK {
                for (px, pz) in [(1, step + 1), (32, step + 1), (step + 1, 1), (step + 1, 32)] {
                    self.seed_cell(channel, px, pz, py);
                }
            }
        }
    }

    fn seed_cell(&mut self, channel: Channel, px: usize, pz: usize, py: usize) {
        let y = i32::try_from(py).expect("a padded y fits i32");
        if self.in_w(px, pz, y) {
            return;
        }
        let level = self.light(channel, padded_cell(px, pz, py));
        if level > 1 && self.improves(channel, px, pz, y, level) {
            self.push(px, pz, y, level);
        }
    }

    /// Whether `level` at this cell can raise a writable neighbour.
    fn improves(&self, channel: Channel, px: usize, pz: usize, y: i32, level: u8) -> bool {
        for (dx, dz, dy) in NEIGHBOURS {
            let Some((nx, nz, ny)) = offset(px, pz, y, dx, dz, dy) else {
                continue;
            };
            if !self.in_w(nx, nz, ny) {
                continue;
            }
            let index = padded_cell(nx, nz, padded(ny));
            let cost = step_cost(channel, self.cells[index], dy);
            if level.saturating_sub(cost) > self.light(channel, index) {
                return true;
            }
        }
        false
    }

    fn push(&mut self, px: usize, pz: usize, y: i32, level: u8) {
        let entry = pack(px, pz, padded(y), level);
        if !self.queue.push(entry) {
            self.overflow = true;
        }
    }

    /// The lateral BFS. Writes only the writable region.
    fn flood(&mut self, channel: Channel) {
        while let Some(entry) = self.queue.pop() {
            let level = unpack_level(entry);
            if level <= 1 {
                continue;
            }
            let (px, pz, py) = unpack_cell(entry);
            match channel {
                Channel::Sky => self.stats.sky_steps += 1,
                Channel::Block => self.stats.block_steps += 1,
            }
            let y = i32::try_from(py).expect("a padded y fits i32");
            for (dx, dz, dy) in NEIGHBOURS {
                let Some((nx, nz, ny)) = offset(px, pz, y, dx, dz, dy) else {
                    continue;
                };
                if !self.in_w(nx, nz, ny) {
                    continue;
                }
                let index = padded_cell(nx, nz, padded(ny));
                let next = level.saturating_sub(step_cost(channel, self.cells[index], dy));
                if next > self.light(channel, index) {
                    self.write(channel, index, next);
                    self.push(nx, nz, ny, next);
                }
            }
        }
        if self.overflow {
            self.stats.queue_overflows += 1;
            self.overflow = false;
            self.sweep(channel);
        }
    }

    /// Bounded relaxation to the same fixed point, used after a queue overflow.
    fn sweep(&mut self, channel: Channel) {
        for _ in 0..=LIGHT_LEVELS {
            let mut changed = false;
            for py in 0..384_usize {
                let y = i32::try_from(py).expect("a padded y fits i32");
                for pz in 0..PADDED_WIDTH {
                    for px in 0..PADDED_WIDTH {
                        if !self.in_w(px, pz, y) {
                            continue;
                        }
                        let index = padded_cell(px, pz, py);
                        let current = self.light(channel, index);
                        let mut best = current;
                        for (dx, dz, dy) in NEIGHBOURS {
                            let Some((nx, nz, ny)) = offset(px, pz, y, dx, dz, dy) else {
                                continue;
                            };
                            let source = self.light(channel, padded_cell(nx, nz, padded(ny)));
                            // The light travels from the neighbour to this cell,
                            // so the step direction is the negated offset.
                            let cost = step_cost(channel, self.cells[index], -dy);
                            best = best.max(source.saturating_sub(cost));
                        }
                        if best > current {
                            self.write(channel, index, best);
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// Reads the interior back into fresh slots.
    fn extract(&self) -> ColumnLight {
        let mut light = ColumnLight::new();
        for section in 0..COLUMN_SECTIONS {
            let base = section * 32;
            let sky = &mut light.sky[section];
            let block = &mut light.block[section];
            for y in 0..=LOCAL_MASK {
                for z in 0..=LOCAL_MASK {
                    for x in 0..=LOCAL_MASK {
                        let cell = section_cell(x, y, z);
                        let index = padded_cell(x + 1, z + 1, base + y);
                        sky.set(cell, self.light(Channel::Sky, index));
                        block.set(cell, self.light(Channel::Block, index));
                    }
                }
            }
        }
        light
    }

    /// Writes the interior back, counting cells whose value changed.
    fn store(&self, light: &mut ColumnLight) -> u32 {
        let mut changed = 0_u32;
        let lo = usize::try_from(self.y_lo.max(0)).unwrap_or(0);
        let hi = usize::try_from(self.y_hi.max(0)).unwrap_or(0);
        for section in 0..COLUMN_SECTIONS {
            let base = section * 32;
            let start = lo.max(base);
            let end = hi.min(base + LOCAL_MASK);
            if end < start {
                continue;
            }
            for y in start..=end {
                let local = y - base;
                for z in 0..=LOCAL_MASK {
                    for x in 0..=LOCAL_MASK {
                        let cell = section_cell(x, local, z);
                        let index = padded_cell(x + 1, z + 1, y);
                        let sky = self.light(Channel::Sky, index);
                        let block = self.light(Channel::Block, index);
                        let mut differs = light.sky[section].get(cell) != sky;
                        if differs {
                            light.sky[section].set(cell, sky);
                        }
                        if light.block[section].get(cell) != block {
                            light.block[section].set(cell, block);
                            differs = true;
                        }
                        changed += u32::from(differs);
                    }
                }
            }
        }
        changed
    }
}

/// The writable `y` range for a batch of edits (`L3.1`).
///
/// Lateral light reaches 15 blocks, so cells more than 15 above an edit cannot
/// change. The vertical sky seed is lossless downward, however: placing a block
/// changes the seed of every cell below it in that column, all the way to the
/// floor. A correct repair therefore always starts at `y = 0` and stops 15 above
/// the highest touched section.
fn relight_bounds(edits: &[Edit]) -> Option<(i32, i32)> {
    let mut high = i32::MIN;
    for edit in edits {
        let section_top = edit.y - edit.y.rem_euclid(32);
        high = high.max(section_top + 31 + RELIGHT_RADIUS);
    }
    if high == i32::MIN {
        return None;
    }
    Some((0, high.min(WORLD_TOP)))
}

/// Index of one padded cell. `px`, `pz ∈ 0..34` and `py ∈ 0..384`.
const fn padded_cell(px: usize, pz: usize, py: usize) -> usize {
    px + PADDED_WIDTH * (pz + PADDED_WIDTH * py)
}

/// The only place the `I1.2` cell formula exists in this module.
const fn section_cell(x: usize, y: usize, z: usize) -> usize {
    (x & LOCAL_MASK) | ((z & LOCAL_MASK) << 5) | ((y & LOCAL_MASK) << 10)
}

/// Padded `x`/`z` from a world coordinate in `-1..=32`.
fn coord(padded_value: usize) -> i32 {
    i32::try_from(padded_value).expect("a padded coordinate fits i32") - 1
}

/// Padded index from a world `y` in `0..384`.
fn padded(y: i32) -> usize {
    usize::try_from(y).expect("a padded y is non-negative")
}

/// Whether a padded `x`/`z` is on the border ring.
const fn is_ring(px: usize, pz: usize) -> bool {
    px == 0 || px == PADDED_WIDTH - 1 || pz == 0 || pz == PADDED_WIDTH - 1
}

/// A neighbour in padded space, or `None` outside the padded column.
fn offset(px: usize, pz: usize, y: i32, dx: i32, dz: i32, dy: i32) -> Option<(usize, usize, i32)> {
    let nx = i32::try_from(px).ok()? + dx;
    let nz = i32::try_from(pz).ok()? + dz;
    let ny = y + dy;
    if !(0..PADDED_AXIS_I).contains(&nx)
        || !(0..PADDED_AXIS_I).contains(&nz)
        || !(0..COLUMN_HEIGHT).contains(&ny)
    {
        return None;
    }
    Some((usize::try_from(nx).ok()?, usize::try_from(nz).ok()?, ny))
}

/// Packs a queue entry: `px | pz << 6 | y << 12 | level << 21`.
fn pack(px: usize, pz: usize, py: usize, level: u8) -> u32 {
    let px = u32::try_from(px).expect("a padded x fits u32");
    let pz = u32::try_from(pz).expect("a padded z fits u32");
    let py = u32::try_from(py).expect("a padded y fits u32");
    px | (pz << 6) | (py << 12) | (u32::from(level & 0x0F) << 21)
}

fn unpack_level(entry: u32) -> u8 {
    u8::try_from((entry >> 21) & 0x0F).expect("a level is four bits")
}

fn unpack_cell(entry: u32) -> (usize, usize, usize) {
    let px = usize::try_from(entry & 0x3F).expect("a padded x is six bits");
    let pz = usize::try_from((entry >> 6) & 0x3F).expect("a padded z is six bits");
    let py = usize::try_from((entry >> 12) & 0x1FF).expect("a padded y is nine bits");
    (px, pz, py)
}

/// Masks a `u64` lane to a nibble.
fn nibble(value: u64) -> u8 {
    u8::try_from(value & 0x0F).expect("a nibble fits u8")
}

/// Replicates a nibble into every lane of a word.
fn splat(value: u8) -> u64 {
    u64::from(value & 0x0F) * 0x1111_1111_1111_1111
}

fn section_cells_u16() -> u16 {
    u16::try_from(SECTION_CELLS).expect("a section volume fits u16")
}

/// `-1`, `0`, or `1` for a coordinate in `-1..=32`.
fn edge(value: i32) -> i32 {
    match value {
        ..=-1 => -1,
        32.. => 1,
        _ => 0,
    }
}

/// The local coordinate of a border cell in the neighbour column.
fn neighbour_local(value: i32, delta: i32) -> u32 {
    match delta.signum() {
        -1 => 31,
        1 => 0,
        _ => u32::try_from(value).expect("an interior coordinate is non-negative"),
    }
}

/// The interior coordinate a missing border mirrors.
fn mirror(value: i32, delta: i32) -> i32 {
    match delta.signum() {
        -1 => 0,
        1 => 31,
        _ => value,
    }
}

/// The border slot for an offset pair.
fn border_slot(dx: i32, dz: i32) -> usize {
    BORDER_OFFSETS
        .iter()
        .position(|(x, z)| *x == dx && *z == dz)
        .unwrap_or(0)
}

/// Material bits: four bits each for sky attenuation, block opacity, emission.
fn packed_props(table: &LightTable, id: BlockId) -> u16 {
    let props = table.props(id);
    u16::from(props.sky_atten & 0x0F)
        | (u16::from(props.block_opacity & 0x0F) << 4)
        | (u16::from(table.emission(id) & 0x0F) << 8)
}

fn sky_atten(cell: u16) -> u8 {
    u8::try_from(cell & 0x0F).expect("a nibble fits u8")
}

fn block_opacity(cell: u16) -> u8 {
    u8::try_from((cell >> 4) & 0x0F).expect("a nibble fits u8")
}

fn emission(cell: u16) -> u8 {
    u8::try_from((cell >> 8) & 0x0F).expect("a nibble fits u8")
}

/// Loss on entering a cell laterally.
fn lateral_cost(channel: Channel, cell: u16) -> u8 {
    let cost = match channel {
        Channel::Sky => sky_atten(cell),
        Channel::Block => block_opacity(cell),
    };
    cost.max(1)
}

/// Loss on entering a cell from the neighbour `dy` away.
///
/// Sky light straight down loses only the destination's attenuation, which is
/// zero for air: the vertical rule applies to light that arrived laterally too,
/// not just to the pass-1 seed, so air keeps its lossless descent.
///
/// Sky light is never let into an attenuating cell sideways. Water takes its sky
/// from the column above, so `WATER_SKY_ATTEN` accumulates per block of depth
/// instead of every cell being refreshed to the surface value by the open air
/// beside it (`L2.4`). Opaque cells already cost the whole range laterally, so
/// only partially attenuating materials — water in the default table — change.
fn step_cost(channel: Channel, cell: u16, dy: i32) -> u8 {
    match channel {
        Channel::Sky if dy < 0 => sky_atten(cell),
        Channel::Sky if dy == 0 && sky_atten(cell) > 0 => LIGHT_LEVELS,
        _ => lateral_cost(channel, cell),
    }
}

/// Samples the interior of one channel.
fn sample(slots: &[LightSlot; COLUMN_SECTIONS], x: i32, y: i32, z: i32, above: u8) -> u8 {
    if y < 0 {
        return 0;
    }
    let Ok(y) = u32::try_from(y) else {
        return above;
    };
    if y >= WORLD_HEIGHT {
        return above;
    }
    let section = usize::try_from(y / SECTION_SIZE).expect("a section index fits usize");
    let local_y = usize::try_from(y % SECTION_SIZE).expect("a local y fits usize");
    let local_x = usize::try_from(x.rem_euclid(32)).expect("a masked x fits usize");
    let local_z = usize::try_from(z.rem_euclid(32)).expect("a masked z fits usize");
    slots[section].get(section_cell(local_x, local_y, local_z))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::{domain, hash64};

    /// Builds all 12 sections of a column from a block function.
    fn column(f: impl Fn(i32, i32, i32) -> BlockId) -> [Section; COLUMN_SECTIONS] {
        std::array::from_fn(|section| section_from(section, &f))
    }

    fn section_from(section: usize, f: &impl Fn(i32, i32, i32) -> BlockId) -> Section {
        let mut blocks = vec![0_u16; SECTION_CELLS].into_boxed_slice();
        let base = i32::try_from(section).expect("a section index fits i32") * 32;
        for y in 0..32_i32 {
            for z in 0..32_i32 {
                for x in 0..32_i32 {
                    let index = section_cell(
                        usize::try_from(x).expect("an interior x is non-negative"),
                        usize::try_from(y).expect("a local y is non-negative"),
                        usize::try_from(z).expect("an interior z is non-negative"),
                    );
                    blocks[index] = f(x, base + y, z);
                }
            }
        }
        let array: Box<[BlockId; SECTION_CELLS]> = blocks
            .try_into()
            .expect("the scratch matches the section volume");
        Section::from_blocks(&array)
    }

    fn column_blocks(f: impl Fn(i32, i32, i32) -> BlockId) -> ColumnBlocks {
        ColumnBlocks::new(column(f), &[None; 8])
    }

    #[test]
    fn open_column_is_full_sky() {
        let blocks = column_blocks(|_, _, _| AIR_ID);
        let light = compute_column(&blocks, &default_table());

        for y in 0..COLUMN_HEIGHT {
            for z in 0..32 {
                for x in 0..32 {
                    assert_eq!(light.sky(x, y, z), 15, "sky {x},{y},{z}");
                    assert_eq!(light.block(x, y, z), 0, "block {x},{y},{z}");
                }
            }
        }
        for section in 0..COLUMN_SECTIONS {
            assert_eq!(light.sky_slot(section).is_uniform(), Some(15));
            assert_eq!(light.block_slot(section).is_uniform(), Some(0));
            assert!(!light.sky_slot(section).is_packed());
        }
    }

    #[test]
    fn padded_section_reads_vertical_neighbours_and_mirrors_edges() {
        let table = default_table();
        let blocks = column_blocks(|_, y, _| if y <= 100 { 1 } else { AIR_ID });
        let light = compute_column(&blocks, &table);
        let padded = light.padded_section(3);

        assert_eq!(
            padded.get(5, 0, 7),
            (light.sky(5, 96, 7), light.block(5, 96, 7)),
            "the interior is the section itself"
        );
        assert_eq!(
            padded.get(5, 32, 7),
            (light.sky(5, 128, 7), light.block(5, 128, 7)),
            "the top pad reads the section above"
        );
        assert_eq!(
            padded.get(5, -1, 7),
            (light.sky(5, 95, 7), light.block(5, 95, 7)),
            "the bottom pad reads the section below"
        );
        assert_eq!(
            padded.get(-1, 5, 7),
            (light.sky(0, 101, 7), light.block(0, 101, 7)),
            "the horizontal pad mirrors the edge row"
        );
        assert_eq!(
            padded.get(32, 5, 7),
            (light.sky(31, 101, 7), light.block(31, 101, 7))
        );
        assert_eq!(
            padded.get(-1, 32, -1),
            (light.sky(0, 128, 0), light.block(0, 128, 0)),
            "a corner combines horizontal mirroring with the vertical neighbour"
        );

        assert_eq!(light.padded_section(0).get(0, -1, 0), (0, 0), "below world");
        assert_eq!(
            light.padded_section(COLUMN_SECTIONS - 1).get(0, 32, 0),
            (LIGHT_LEVELS, 0),
            "above world"
        );
    }

    #[test]
    fn roof_shadows_the_column() {
        let blocks = column_blocks(|_, y, _| if y == 100 { 1 } else { AIR_ID });
        let light = compute_column(&blocks, &default_table());

        for y in 0..COLUMN_HEIGHT {
            for z in 0..32 {
                for x in 0..32 {
                    let expected = if y > 100 { 15 } else { 0 };
                    assert_eq!(light.sky(x, y, z), expected, "sky {x},{y},{z}");
                    assert_eq!(light.block(x, y, z), 0, "block {x},{y},{z}");
                }
            }
        }
    }

    #[test]
    fn vertical_propagation_has_no_loss() {
        let blocks = column_blocks(|x, y, z| {
            if x == 16 && z == 16 {
                AIR_ID
            } else if y <= 200 {
                1
            } else {
                AIR_ID
            }
        });
        let light = compute_column(&blocks, &default_table());

        for y in 0..=200 {
            assert_eq!(light.sky(16, y, 16), 15, "shaft at {y}");
        }
        assert_eq!(light.sky(15, 100, 16), 0, "solid below the top");
        assert_eq!(light.sky(17, 100, 16), 0);
        assert_eq!(light.sky(16, 201, 16), 15);
    }

    #[test]
    fn lateral_decay_is_one_per_step() {
        let blocks = column_blocks(|x, y, z| {
            if y >= 202 {
                AIR_ID
            } else if y == 201 {
                if x == 0 && z == 16 { AIR_ID } else { 1 }
            } else if y == 200 {
                if z == 16 && x <= 10 { AIR_ID } else { 1 }
            } else {
                1
            }
        });
        let light = compute_column(&blocks, &default_table());

        for x in 0..=10 {
            assert_eq!(
                light.sky(x, 200, 16),
                15 - u8::try_from(x).expect("x fits u8"),
                "tunnel at x={x}"
            );
        }
        for x in 11..32 {
            assert_eq!(light.sky(x, 200, 16), 0, "dead end at x={x}");
        }
    }

    #[test]
    fn water_attenuates_sky_only() {
        let blocks = column_blocks(|x, y, z| {
            if x != 16 || z != 16 {
                return if y <= 111 { 1 } else { AIR_ID };
            }
            match y {
                99 => LAMP_ID,
                100..=110 => WATER_ID,
                _ => AIR_ID,
            }
        });
        let light = compute_column(&blocks, &default_table());

        assert_eq!(light.sky(16, 111, 16), 15, "air above the water");
        for y in 100..=110 {
            let depth = u8::try_from(111 - y).expect("depth fits u8");
            assert_eq!(light.sky(16, y, 16), 15 - depth, "sky at {y}");
        }
        for y in 99..=110 {
            let step = u8::try_from(y - 99).expect("step fits u8");
            assert_eq!(light.block(16, y, 16), 14 - step, "block at {y}");
        }
    }

    #[test]
    fn vertical_sky_loses_water_attenuation_per_block() {
        // Four water blocks on the stone surface, open to the sky and with open
        // air beside them. If a sideways step could enter water, each cell would
        // be refreshed to 14 from its neighbour; `L2.4` says the depth charge
        // must survive.
        let blocks = column_blocks(|x, y, z| {
            if x == 16 && z == 16 && (100..=103).contains(&y) {
                WATER_ID
            } else if y <= 99 {
                1
            } else {
                AIR_ID
            }
        });
        let light = compute_column(&blocks, &default_table());

        assert_eq!(light.sky(16, 104, 16), 15, "air above the column");
        assert_eq!(light.sky(16, 103, 16), 14, "the surface water");
        assert_eq!(light.sky(16, 102, 16), 13);
        assert_eq!(light.sky(16, 101, 16), 12);
        assert_eq!(light.sky(16, 100, 16), 11, "the deepest water");
        assert_eq!(light.sky(16, 99, 16), 0, "the block under the column");
    }

    #[test]
    fn water_column_is_darker_with_depth() {
        let bottom = |depth: i32| {
            let blocks = column_blocks(|x, y, z| {
                if x == 16 && z == 16 && ((104 - depth)..=103).contains(&y) {
                    WATER_ID
                } else if y <= 99 {
                    1
                } else {
                    AIR_ID
                }
            });
            let light = compute_column(&blocks, &default_table());
            light.sky(16, 104 - depth, 16)
        };

        let shallow = bottom(1);
        let deep = bottom(4);
        assert_eq!(shallow, 14, "the bottom of a 1-deep column");
        assert_eq!(deep, 11, "the bottom of a 4-deep column");
        assert_eq!(
            shallow - deep,
            3,
            "three extra water blocks cost three levels"
        );
    }

    #[test]
    fn incremental_equals_full_recompute() {
        const EDITS: u64 = 20_000;
        const BATCH: usize = 500;
        let tag = domain("light2:fuzz");
        let table = default_table();
        let mut blocks = column_blocks(|_, y, _| if y <= 120 { 1 } else { AIR_ID });
        let mut light = compute_column(&blocks, &table);
        let mut edits = Vec::with_capacity(BATCH);
        let mut changed_batches = 0_u32;

        for step in 0..EDITS {
            let hash = hash64(7, tag, &[i64::try_from(step).expect("step fits i64")]);
            let x = i32::try_from(hash % 32).expect("x fits");
            let z = i32::try_from((hash >> 8) % 32).expect("z fits");
            let y = i32::try_from((hash >> 16) % 200).expect("y fits");
            let id = match (hash >> 32) % 7 {
                0 => AIR_ID,
                1 => WATER_ID,
                2 => LEAVES_ID,
                3 => LAMP_ID,
                4 => 1,
                5 => 3,
                _ => 7,
            };
            blocks.set(x, y, z, id);
            edits.push(Edit::new(x, y, z, id));
            let last = step + 1 == EDITS;
            if edits.len() < BATCH && !last {
                continue;
            }

            let before = light.clone();
            let changed = light.relight_column(&blocks, &table, &edits);
            let fresh = compute_column(&blocks, &table);
            assert_eq!(light, fresh, "incremental diverged at edit {step}");
            assert_eq!(
                light.packed_bytes(),
                fresh.packed_bytes(),
                "slot representation diverged at edit {step}"
            );
            assert_eq!(
                changed,
                diff_count(&before, &light),
                "changed count at edit {step}"
            );
            if changed > 0 {
                changed_batches += 1;
            }
            edits.clear();
        }

        assert!(changed_batches > 0, "the fuzz never changed light");
        assert_eq!(
            light.relight_column(&blocks, &table, &[]),
            0,
            "an empty batch is a no-op"
        );
    }

    fn diff_count(before: &ColumnLight, after: &ColumnLight) -> u32 {
        let mut changed = 0_u32;
        for section in 0..COLUMN_SECTIONS {
            for cell in 0..SECTION_CELLS {
                if before.sky_slot(section).get(cell) != after.sky_slot(section).get(cell)
                    || before.block_slot(section).get(cell) != after.block_slot(section).get(cell)
                {
                    changed += 1;
                }
            }
        }
        changed
    }

    #[test]
    fn section_differs_names_only_the_changed_section() {
        let table = default_table();
        let before = compute_column(
            &column_blocks(|_, y, _| if y <= 120 { 1 } else { AIR_ID }),
            &table,
        );
        let after = compute_column(
            &column_blocks(|x, y, z| {
                let top = if x == 5 && z == 5 { 121 } else { 120 };
                if y <= top { 1 } else { AIR_ID }
            }),
            &table,
        );

        assert!(
            before.section_differs(&after, 3),
            "section 3 holds the change"
        );
        for section in (0..COLUMN_SECTIONS).filter(|section| *section != 3) {
            assert!(
                !before.section_differs(&after, section),
                "section {section} is unchanged"
            );
        }
        assert!(!before.section_differs(&before.clone(), 3));
    }

    #[test]
    fn lateral_light_falls_straight_down_without_loss() {
        // A roofed vertical shaft is lit only through a side tunnel at y=300.
        // Sky light that arrives sideways must still run down the shaft with no
        // loss, exactly like the pass-1 seed.
        let blocks = column_blocks(|x, y, z| {
            if z != 16 {
                return 1;
            }
            match (x, y) {
                (16, 100..=382) | (17..=20, 300) | (20, 300..=383) => AIR_ID,
                _ => 1,
            }
        });
        let light = compute_column(&blocks, &default_table());

        assert_eq!(light.sky(20, 300, 16), 15, "the open shaft");
        assert_eq!(light.sky(19, 300, 16), 14);
        assert_eq!(light.sky(16, 300, 16), 11);
        for y in 100..=300 {
            assert_eq!(light.sky(16, y, 16), 11, "shaft at {y}");
        }
        assert_eq!(light.sky(16, 99, 16), 0, "the floor");
    }

    #[test]
    fn uniform_promotion_and_demotion() {
        let table = default_table();
        let mut blocks = column_blocks(|_, _, _| AIR_ID);
        let mut light = compute_column(&blocks, &table);
        assert!(light.is_uniform());

        // One dark cell reallocates the touched section. Open air below the
        // block reads 14 from its neighbours, so a whole section below it
        // becomes a uniform 14 slot.
        blocks.set(16, 200, 16, 1);
        let before = light.clone();
        let edits = [Edit::new(16, 200, 16, 1)];
        let changed = light.relight_column(&blocks, &table, &edits);
        assert_eq!(changed, diff_count(&before, &light));
        assert_eq!(light.sky(16, 200, 16), 0);
        assert_eq!(light.sky(16, 199, 16), 14);
        assert!(light.sky_slot(6).is_packed());
        assert_eq!(light.sky_slot(6).is_uniform(), None);
        assert_eq!(light.sky_slot(6).get(section_cell(16, 8, 16)), 0);
        assert_eq!(light.sky(16, 10, 16), 14);
        assert_eq!(light.sky(15, 10, 16), 15);

        blocks.set(16, 200, 16, AIR_ID);
        let edits = [Edit::new(16, 200, 16, AIR_ID)];
        light.relight_column(&blocks, &table, &edits);
        assert!(light.is_uniform(), "the section must collapse again");
        assert_eq!(light.sky_slot(6).is_uniform(), Some(15));
        assert!(!light.sky_slot(6).is_packed());
    }

    #[test]
    fn border_light_crosses_the_seam() {
        let a_sections = column(|x, y, z| {
            let shaft = x == 31 && z == 16;
            if y >= 100 || shaft { AIR_ID } else { 1 }
        });
        let b_sections = column(|x, y, z| {
            let shaft = x == 0 && z == 16;
            if y >= 100 || shaft { AIR_ID } else { 1 }
        });
        let a_copy = a_sections.clone();
        let mut borders_a: [Option<&[Section; COLUMN_SECTIONS]>; 8] = [None; 8];
        borders_a[4] = Some(&b_sections);
        let a = ColumnBlocks::new(a_sections, &borders_a);
        let mut borders_b: [Option<&[Section; COLUMN_SECTIONS]>; 8] = [None; 8];
        borders_b[3] = Some(&a_copy);
        let b = ColumnBlocks::new(b_sections, &borders_b);

        let table = default_table();
        let light_a = compute_column(&a, &table);
        let light_b = compute_column(&b, &table);

        for y in 0..COLUMN_HEIGHT {
            for z in 0..32 {
                assert_eq!(
                    light_a.sky(31, y, z),
                    light_b.sky(0, y, z),
                    "seam disagreement at {y},{z}"
                );
                assert_eq!(light_a.block(31, y, z), light_b.block(0, y, z));
            }
        }
        assert_eq!(light_a.sky(31, 50, 16), 15, "the shared shaft is open");
        assert_eq!(light_b.sky(0, 50, 16), 15);
        assert_eq!(light_a.sky(31, 50, 17), 0, "and the wall is dark");
    }

    #[test]
    fn queue_has_a_fixed_capacity() {
        let mut ring = Ring::new();
        for entry in 0..u32::try_from(QUEUE_CAPACITY).expect("the capacity fits u32") {
            assert!(ring.push(entry), "push {entry}");
        }
        assert!(!ring.push(0), "a full ring refuses the push");
        assert_eq!(ring.pop(), Some(0));
        assert!(ring.push(0), "a pop makes room again");
        assert_eq!(ring.pop(), Some(1));
    }

    #[test]
    fn top_span_skips_open_air() {
        let table = default_table();
        let mut blocks = column_blocks(|_, y, _| if y <= 8 { 1 } else { AIR_ID });

        let (plain, plain_stats) = compute_column_stats(&blocks, &table);
        assert_eq!(plain_stats.vertical_cells, PADDED_CELLS);

        blocks.set_top_span(8);
        let (hinted, hinted_stats) = compute_column_stats(&blocks, &table);
        assert_eq!(hinted, plain, "the hint must not change light");
        assert!(
            hinted_stats.vertical_cells < 100_000,
            "the hint visited {} cells",
            hinted_stats.vertical_cells
        );
        assert!(hinted_stats.vertical_cells > 0);
    }
}
