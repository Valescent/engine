//! Column light storage, block access, and the full-column build.

use crate::description::{SECTION_SIZE, SECTIONS_PER_COLUMN, WORLD_HEIGHT};
use crate::padded::{LOCAL_MAX, LOCAL_MIN, PaddedLight};
use crate::section::{BlockId, Section};

use super::bfs::{Channel, LOCAL_MASK, Scratch};
use super::repair::{ColumnView, Edit, relight_bounds};
use super::table::{AIR_ID, BEDROCK_ID, LIGHT_LEVELS, LightTable};

/// Sections per column (`I1.3`).
pub const COLUMN_SECTIONS: usize = 12;

/// Cells in one section.
pub const SECTION_CELLS: usize = 32_768;

/// Height of a whole column in blocks (`C4.3`).
pub const COLUMN_HEIGHT: i32 = 384;

/// `u64` words in one packed section.
pub const NIBBLE_WORDS: usize = 2048;

pub(super) const WORLD_TOP: i32 = COLUMN_HEIGHT - 1;

const _: () = assert!(SECTIONS_PER_COLUMN == 12);
const _: () = assert!(SECTION_SIZE == 32);
const _: () = assert!(WORLD_HEIGHT == 384);

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
    pub(super) sky: [LightSlot; COLUMN_SECTIONS],
    pub(super) block: [LightSlot; COLUMN_SECTIONS],
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

/// The only place the `I1.2` cell formula exists in this module.
pub(super) const fn section_cell(x: usize, y: usize, z: usize) -> usize {
    (x & LOCAL_MASK) | ((z & LOCAL_MASK) << 5) | ((y & LOCAL_MASK) << 10)
}

/// Masks a `u64` lane to a nibble.
pub(super) fn nibble(value: u64) -> u8 {
    u8::try_from(value & 0x0F).expect("a nibble fits u8")
}

/// Replicates a nibble into every lane of a word.
pub(super) fn splat(value: u8) -> u64 {
    u64::from(value & 0x0F) * 0x1111_1111_1111_1111
}

fn section_cells_u16() -> u16 {
    u16::try_from(SECTION_CELLS).expect("a section volume fits u16")
}

/// `-1`, `0`, or `1` for a coordinate in `-1..=32`.
pub(super) fn edge(value: i32) -> i32 {
    match value {
        ..=-1 => -1,
        32.. => 1,
        _ => 0,
    }
}

/// The local coordinate of a border cell in the neighbour column.
pub(super) fn neighbour_local(value: i32, delta: i32) -> u32 {
    match delta.signum() {
        -1 => 31,
        1 => 0,
        _ => u32::try_from(value).expect("an interior coordinate is non-negative"),
    }
}

/// The interior coordinate a missing border mirrors.
pub(super) fn mirror(value: i32, delta: i32) -> i32 {
    match delta.signum() {
        -1 => 0,
        1 => 31,
        _ => value,
    }
}

/// The border slot for an offset pair.
pub(super) fn border_slot(dx: i32, dz: i32) -> usize {
    BORDER_OFFSETS
        .iter()
        .position(|(x, z)| *x == dx && *z == dz)
        .unwrap_or(0)
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
