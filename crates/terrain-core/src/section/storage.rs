//! `W1` section storage: the 32³ block array, block ids, and index mapping.
//!
//! A section is 32 blocks on every axis and the cell order is
//! `x | z << 5 | y << 10`, so `x` is fastest and one `y` layer is contiguous.
//! `section_index` and `section_coord` are the only places that formula
//! exists; every loop calls them instead of inlining the shifts.
//!
//! A block is a [`BlockId`]. Id `0` is air by registry convention, which is what
//! the stored solid count tracks.
//!
//! `Section::Uniform` stores one id for the whole volume.
//! [`Section::Paletted`] stores a palette and bit-packed palette indices, `bits`
//! per cell, LSB-first inside `u64` words. Every lane width divides 64, so a
//! field never straddles a word boundary.
//!
//! The palette is unordered and append-only. Nothing downstream may branch on
//! palette order: decoding, equality, and [`Section::content_hash`] all go
//! through block ids. Growing the palette repacks in one pass. Shrinking back to
//! one id is [`Section::compact`]'s explicit job, never a side effect of a write.

use std::collections::BTreeSet;
use std::fmt;

use crate::description::{SECTION_SIZE, SECTION_VOLUME};

/// A block type id. Id `0` is air by registry convention.
pub type BlockId = u16;

/// Local coordinate mask (`31`).
const LOCAL_MASK: u32 = SECTION_SIZE - 1;

/// Bit position of the `z` field in a section index.
const LOCAL_SHIFT: u32 = SECTION_SIZE.trailing_zeros();

/// The store for one 32³ section.
#[derive(Clone)]
pub enum Section {
    /// One id for every cell.
    Uniform(BlockId),
    /// A palette plus bit-packed palette indices.
    Paletted {
        /// Distinct block ids in append-only discovery order.
        palette: Vec<BlockId>,
        /// Bits per cell: `1`, `2`, `4`, `8`, or `16`.
        bits: u8,
        /// Packed indices, LSB-first, `SECTION_VOLUME * bits / 64` words.
        data: Box<[u64]>,
        /// Cells whose id is not `0`, maintained by every write.
        solid: u32,
    },
}

impl Section {
    /// A section filled with one id.
    #[must_use]
    pub fn uniform(id: BlockId) -> Self {
        Self::Uniform(id)
    }

    /// Builds a section from a full block array, choosing the smallest width.
    #[must_use]
    pub fn from_blocks(blocks: &[BlockId; SECTION_VOLUME]) -> Self {
        let mut distinct = BTreeSet::new();
        for id in blocks {
            distinct.insert(*id);
        }
        if distinct.len() == 1
            && let Some(id) = distinct.first().copied()
        {
            return Self::uniform(id);
        }

        let palette: Vec<BlockId> = distinct.into_iter().collect();
        let bits = bits_for(palette.len());
        let mut data = vec![0_u64; word_count(bits)].into_boxed_slice();
        let mut solid = 0_u32;
        for (index, id) in blocks.iter().enumerate() {
            let slot = palette
                .binary_search(id)
                .expect("the palette holds every block in the array");
            write_word(&mut data, index, bits, slot);
            solid += u32::from(*id != 0);
        }
        Self::Paletted {
            palette,
            bits,
            data,
            solid,
        }
    }

    /// Reads one cell. Coordinates are masked to `0..32`.
    #[must_use]
    pub fn get(&self, x: u32, y: u32, z: u32) -> BlockId {
        self.get_index(section_index(x, y, z))
    }

    /// Writes one cell. Coordinates are masked to `0..32`.
    pub fn set(&mut self, x: u32, y: u32, z: u32, id: BlockId) {
        self.set_index(section_index(x, y, z), id);
    }

    /// Reads one cell by `section_index`. Indices are masked to the volume.
    #[must_use]
    pub fn get_index(&self, index: usize) -> BlockId {
        let index = index & (SECTION_VOLUME - 1);
        match self {
            Self::Uniform(id) => *id,
            Self::Paletted {
                palette,
                bits,
                data,
                ..
            } => palette[read_word(data, index, *bits)],
        }
    }

    /// Writes one cell by `section_index`. Indices are masked to the volume.
    pub(crate) fn set_index(&mut self, index: usize, id: BlockId) {
        let index = index & (SECTION_VOLUME - 1);
        match self {
            Self::Uniform(current) => {
                let current = *current;
                if current == id {
                    return;
                }
                let bits = bits_for(2);
                let mut data = vec![0_u64; word_count(bits)].into_boxed_slice();
                write_word(&mut data, index, bits, 1);
                let solid = if current == 0 {
                    u32::from(id != 0)
                } else {
                    volume_u32() - 1 + u32::from(id != 0)
                };
                *self = Self::Paletted {
                    palette: vec![current, id],
                    bits,
                    data,
                    solid,
                };
            }
            Self::Paletted {
                palette,
                bits,
                data,
                solid,
            } => {
                let slot = palette
                    .iter()
                    .position(|entry| *entry == id)
                    .unwrap_or_else(|| {
                        let slot = palette.len();
                        palette.push(id);
                        let needed = bits_for(palette.len());
                        if needed > *bits {
                            *data = repack(data, *bits, needed);
                            *bits = needed;
                        }
                        slot
                    });
                let previous = palette[read_word(data, index, *bits)];
                write_word(data, index, *bits, slot);
                *solid = *solid - u32::from(previous != 0) + u32::from(id != 0);
            }
        }
    }

    /// Decodes the whole section, the only bulk decode path.
    pub(crate) fn write_blocks(&self, out: &mut [BlockId; SECTION_VOLUME]) {
        match self {
            Self::Uniform(id) => out.fill(*id),
            Self::Paletted { .. } => {
                for (index, cell) in out.iter_mut().enumerate() {
                    *cell = self.get_index(index);
                }
            }
        }
    }

    /// The palette in append-only discovery order.
    ///
    /// A `Uniform` section reports its one id, so this is never empty.
    #[must_use]
    pub fn palette(&self) -> &[BlockId] {
        match self {
            Self::Uniform(id) => std::slice::from_ref(id),
            Self::Paletted { palette, .. } => palette,
        }
    }

    /// Bits per cell. Always the smallest width that fits [`Self::palette`].
    #[must_use]
    pub(crate) fn bits(&self) -> u8 {
        bits_for(self.palette().len())
    }

    /// The uniform id, or `None` when the section is paletted.
    ///
    /// A paletted section is never demoted implicitly, so this stays `None`
    /// until `Self::compact` runs.
    #[must_use]
    pub fn is_uniform(&self) -> Option<BlockId> {
        match self {
            Self::Uniform(id) => Some(*id),
            Self::Paletted { .. } => None,
        }
    }

    /// Number of cells whose id is not `0`, maintained in `O(1)`.
    #[must_use]
    pub(crate) fn solid_count(&self) -> usize {
        match self {
            Self::Uniform(id) => {
                if *id == 0 {
                    0
                } else {
                    SECTION_VOLUME
                }
            }
            Self::Paletted { solid, .. } => {
                usize::try_from(*solid).expect("a section count fits usize")
            }
        }
    }

    /// Whether every cell equals `air`.
    ///
    /// `O(1)` for the reserved air id `0`, which the solid count tracks. A
    /// palette is append-only, so any other id is answered by decoding.
    #[must_use]
    pub(crate) fn is_empty(&self, air: BlockId) -> bool {
        match self {
            Self::Uniform(id) => *id == air,
            Self::Paletted { .. } if air == 0 => self.solid_count() == 0,
            Self::Paletted { .. } => (0..SECTION_VOLUME).all(|index| self.get_index(index) == air),
        }
    }

    /// Demotes a section whose cells all hold one id to `Self::Uniform`.
    ///
    /// The palette is append-only, so stale entries can outnumber the ids still
    /// in use; this scans the decoded cells and ignores them. Anything else is a
    /// no-op. Call it lazily, on save or a timer, never on every write.
    pub(crate) fn compact(&mut self) {
        let Self::Paletted {
            palette,
            bits,
            data,
            ..
        } = self
        else {
            return;
        };
        let first = palette[read_word(data, 0, *bits)];
        let mut single = true;
        for index in 1..SECTION_VOLUME {
            if palette[read_word(data, index, *bits)] != first {
                single = false;
                break;
            }
        }
        if single {
            *self = Self::uniform(first);
        }
    }

    /// The palette as an owned copy, for the save layer.
    #[must_use]
    pub(crate) fn palette_snapshot(&self) -> Vec<BlockId> {
        self.palette().to_vec()
    }

    /// The packed words, for the save layer. Empty for a `Uniform` section.
    #[must_use]
    pub fn words(&self) -> &[u64] {
        match self {
            Self::Uniform(_) => &[],
            Self::Paletted { data, .. } => data,
        }
    }

    /// Rebuilds a paletted section from a saved palette, width, and words.
    ///
    /// Rejects a palette or width that breaks the invariants of `I1.1`: the
    /// palette must be non-empty and unique, `bits` must be the smallest allowed
    /// width for it, the word count must match, and every decoded index must be
    /// inside the palette. The result is never demoted; call `Self::compact`.
    pub fn from_palette(palette: &[BlockId], bits: u8, data: &[u64]) -> Result<Self, &'static str> {
        if palette.is_empty() {
            return Err("palette is empty");
        }
        if !matches!(bits, 1 | 2 | 4 | 8 | 16) {
            return Err("bits must be 1, 2, 4, 8, or 16");
        }
        if bits != bits_for(palette.len()) {
            return Err("bits is not the smallest width for the palette");
        }
        if data.len() != word_count(bits) {
            return Err("data word count does not match bits");
        }
        let mut seen = BTreeSet::new();
        for id in palette {
            if !seen.insert(*id) {
                return Err("palette has duplicate ids");
            }
        }
        let mut solid = 0_u32;
        for index in 0..SECTION_VOLUME {
            let slot = read_word(data, index, bits);
            if slot >= palette.len() {
                return Err("decoded index is outside the palette");
            }
            solid += u32::from(palette[slot] != 0);
        }
        Ok(Self::Paletted {
            palette: palette.to_vec(),
            bits,
            data: data.into(),
            solid,
        })
    }
}

impl PartialEq for Section {
    /// Content equality: two sections are equal when they hold the same ids,
    /// whatever palette order or variant either one uses.
    fn eq(&self, other: &Self) -> bool {
        if self.solid_count() != other.solid_count() {
            return false;
        }
        if let (Self::Uniform(left), Self::Uniform(right)) = (self, other) {
            return left == right;
        }
        (0..SECTION_VOLUME).all(|index| self.get_index(index) == other.get_index(index))
    }
}

impl Eq for Section {}

impl fmt::Debug for Section {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Uniform(id) => formatter.debug_tuple("Uniform").field(id).finish(),
            Self::Paletted {
                palette,
                bits,
                solid,
                ..
            } => formatter
                .debug_struct("Paletted")
                .field("palette_len", &palette.len())
                .field("bits", bits)
                .field("solid", solid)
                .finish_non_exhaustive(),
        }
    }
}

/// The persisted identity of one saved section (`§3.2`).
///
/// This is the on-disk key: a column and a section slot. The revision carried by
/// `crate::revisions::SectionKey` is runtime state and is never saved, so the
/// save layer uses this smaller type.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SectionKey {
    /// The column, `[x, z]`.
    pub column: [i32; 2],
    /// Section slot in the column, `0..12`.
    pub section: u8,
}

/// Index of a local cell in a section.
///
/// The only place the formula exists. Coordinates are masked to `0..32`, so a
/// caller may pass a world coordinate and address `(x & 31, y & 31, z & 31)`.
#[must_use]
pub(crate) fn section_index(x: u32, y: u32, z: u32) -> usize {
    ((x & LOCAL_MASK) | ((z & LOCAL_MASK) << LOCAL_SHIFT) | ((y & LOCAL_MASK) << (2 * LOCAL_SHIFT)))
        as usize
}

/// The `(x, y, z)` local cell of a section index.
///
/// The inverse of `section_index`. Indices are masked to the volume.
#[must_use]
pub(crate) fn section_coord(index: usize) -> (u32, u32, u32) {
    let index = u32::try_from(index & (SECTION_VOLUME - 1)).expect("a masked index fits u32");
    let x = index & LOCAL_MASK;
    let z = (index >> LOCAL_SHIFT) & LOCAL_MASK;
    let y = (index >> (2 * LOCAL_SHIFT)) & LOCAL_MASK;
    (x, y, z)
}

/// Smallest allowed lane width that holds `entries` palette entries.
///
/// A section has at most 65,536 block types, so `16` is the ceiling.
pub(super) const fn bits_for(entries: usize) -> u8 {
    match entries {
        0..=2 => 1,
        3..=4 => 2,
        5..=16 => 4,
        17..=256 => 8,
        _ => 16,
    }
}

/// Lanes that fit in one `u64` at `bits` per lane.
#[allow(clippy::cast_lossless, reason = "a const fn cannot call usize::from")]
pub(super) const fn lanes_per_word(bits: u8) -> usize {
    (64 / bits) as usize
}

/// Words a packed section needs at `bits` per lane.
pub(super) const fn word_count(bits: u8) -> usize {
    SECTION_VOLUME.div_ceil(lanes_per_word(bits))
}

/// The lane value mask for `bits` per lane.
pub(super) const fn mask_for(bits: u8) -> u64 {
    (1_u64 << bits) - 1
}

/// Word and bit offset of one lane. Every lane sits wholly inside one word.
pub(super) const fn lane(index: usize, bits: u8) -> (usize, usize) {
    let per_word = lanes_per_word(bits);
    (index / per_word, (index % per_word) * (bits as usize))
}

/// Reads one packed lane.
pub(super) fn read_word(data: &[u64], index: usize, bits: u8) -> usize {
    let (word, shift) = lane(index, bits);
    usize::try_from((data[word] >> shift) & mask_for(bits)).expect("a lane fits usize")
}

/// Writes one packed lane, leaving every other bit of its word untouched.
pub(super) fn write_word(data: &mut [u64], index: usize, bits: u8, value: usize) {
    let (word, shift) = lane(index, bits);
    let mask = mask_for(bits) << shift;
    let value = u64::try_from(value).expect("a palette slot fits u64") << shift;
    data[word] = (data[word] & !mask) | (value & mask);
}

/// Repacks every lane from `from_bits` to `to_bits` in one pass.
fn repack(data: &[u64], from_bits: u8, to_bits: u8) -> Box<[u64]> {
    let mut out = vec![0_u64; word_count(to_bits)].into_boxed_slice();
    for index in 0..SECTION_VOLUME {
        let value = read_word(data, index, from_bits);
        write_word(&mut out, index, to_bits, value);
    }
    out
}

/// [`SECTION_VOLUME`] as the count type.
pub(super) fn volume_u32() -> u32 {
    u32::try_from(SECTION_VOLUME).expect("a section volume fits u32")
}
