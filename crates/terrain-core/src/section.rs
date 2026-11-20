//! `W1` section storage: a 32³ block array stored as `Uniform` or `Paletted`.
//!
//! `I1.1`, `I1.2`. A section is 32 blocks on every axis and the cell order is
//! `x | z << 5 | y << 10`, so `x` is fastest and one `y` layer is contiguous.
//! [`section_index`] and [`section_coord`] are the only places that formula
//! exists; every loop calls them instead of inlining the shifts.
//!
//! A block is a [`BlockId`]. Id `0` is air by registry convention, which is what
//! the stored solid count tracks.
//!
//! [`Section::Uniform`] stores one id for the whole volume.
//! [`Section::Paletted`] stores a palette and bit-packed palette indices, `bits`
//! per cell, LSB-first inside `u64` words. Every lane width divides 64, so a
//! field never straddles a word boundary.
//!
//! The palette is unordered and append-only. Nothing downstream may branch on
//! palette order: decoding, equality, and [`Section::content_hash`] all go
//! through block ids. Growing the palette repacks in one pass. Shrinking back to
//! one id is [`Section::compact`]'s explicit job, never a side effect of a write.
//!
//! # Saved record (`J4.1`, `J4.2`, `J4.4`)
//!
//! [`encode_record`] and [`decode_record`] define the canonical on-disk section
//! payload. It stores **stable block names**, never runtime ids (`A5.1`,
//! `T1.6`): the encoder maps ids through a [`ContentPack`] and the decoder maps
//! names back. All integers are little-endian.
//!
//! | Offset | Width | Field |
//! |---|---|---|
//! | `0..4` | 4 | magic `"TSEC"` |
//! | `4..6` | 2 | record version: [`SECTION_RECORD_VERSION`] |
//! | `6..10` | 4 | column `x: i32` |
//! | `10..14` | 4 | column `z: i32` |
//! | `14..15` | 1 | section slot `y: u8` (`0..12`) |
//! | `15..16` | 1 | flags: bit 0 `edited`; every other bit must be zero |
//! | `16..48` | 32 | `baseline_hash: [u8; 32]` |
//! | `48..49` | 1 | kind: `0` uniform, `1` paletted |
//! | `49..53` | 4 | solid count (`u32`): cells whose id is not `0` |
//! | `53..` | … | body |
//! | last 4 | 4 | `crc32` over every preceding byte |
//!
//! Body for `Uniform`: one name (`u16` byte length, then UTF-8 bytes).
//!
//! Body for `Paletted`:
//!
//! ```text
//! bits: u8                1, 2, 4, 8, or 16
//! count: u32              palette entries, 1..=65,536
//! count × name            u16 length, then UTF-8 bytes
//! words                   SECTION_VOLUME * bits / 64 u64 lanes
//! ```
//!
//! The palette is sorted by **name**, not declaration order, and the index
//! words are repacked on write so a lane is the position in that sorted list.
//! That makes the index values a function of the name set alone: two sections
//! with equal content encode to identical bytes whatever order their runtime
//! palettes were discovered in. The packed lanes are exactly `I1.1`: LSB-first,
//! `bits` per lane, and never straddling a `u64`; the word count is derived from
//! `bits`, so a record cannot claim a shape its payload does not have.
//!
//! `crc32` is the IEEE reflected polynomial `0xEDB8_8320` already used by
//! `header.bin`; this module reuses [`crate::header::crc32`] rather than
//! duplicating it.
//!
//! A name the pack does not hold decodes to a placeholder id starting at
//! [`UNKNOWN_BASE`] — one per distinct unknown name, in palette order — and is
//! reported in [`SavedSection::unknown_names`]. It never fails the record. The
//! placeholder range is finite, so a section with more distinct unknown names
//! than it can hold is a `ManifestIncompatible` manifest failure rather than a
//! corrupt record. A core name missing from the pack, a bad palette, an
//! impossible bit width, an out-of-range index, a truncated payload, or a bad
//! checksum is a hard error.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use crate::content::{ContentPack, MAX_NAME_BYTES, UNKNOWN_BASE};
use crate::description::{SECTION_SIZE, SECTION_VOLUME, SECTIONS_PER_COLUMN};
use crate::error::{ErrorKind, SectionFault, Stage, TerrainError};
use crate::hash::mix;
use crate::header::crc32;

/// A block type id. Id `0` is air by registry convention.
pub type BlockId = u16;

/// Seed of [`Section::content_hash`].
///
/// Changing it changes every baseline hash, so it is part of the save contract.
const CONTENT_HASH_SEED: u64 = 0x5345_4354_494f_4e00;

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

    /// Reads one cell by [`section_index`]. Indices are masked to the volume.
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

    /// Writes one cell by [`section_index`]. Indices are masked to the volume.
    pub fn set_index(&mut self, index: usize, id: BlockId) {
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
    pub fn write_blocks(&self, out: &mut [BlockId; SECTION_VOLUME]) {
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
    pub fn bits(&self) -> u8 {
        bits_for(self.palette().len())
    }

    /// The uniform id, or `None` when the section is paletted.
    ///
    /// A paletted section is never demoted implicitly, so this stays `None`
    /// until [`Self::compact`] runs.
    #[must_use]
    pub fn is_uniform(&self) -> Option<BlockId> {
        match self {
            Self::Uniform(id) => Some(*id),
            Self::Paletted { .. } => None,
        }
    }

    /// Number of cells whose id is not `0`, maintained in `O(1)`.
    #[must_use]
    pub fn solid_count(&self) -> usize {
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
    pub fn is_empty(&self, air: BlockId) -> bool {
        match self {
            Self::Uniform(id) => *id == air,
            Self::Paletted { .. } if air == 0 => self.solid_count() == 0,
            Self::Paletted { .. } => (0..SECTION_VOLUME).all(|index| self.get_index(index) == air),
        }
    }

    /// Demotes a section whose cells all hold one id to [`Self::Uniform`].
    ///
    /// The palette is append-only, so stale entries can outnumber the ids still
    /// in use; this scans the decoded cells and ignores them. Anything else is a
    /// no-op. Call it lazily, on save or a timer, never on every write.
    pub fn compact(&mut self) {
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

    /// A stable hash of the content, independent of palette order.
    ///
    /// Folds the sorted palette first, then the decoded ids in index order. Two
    /// sections with the same blocks hash equally however their palettes were
    /// discovered, which is what `I2.1` baselines.
    #[must_use]
    pub fn content_hash(&self) -> u64 {
        let mut hash = mix(CONTENT_HASH_SEED);
        match self {
            Self::Uniform(id) => {
                hash = mix(hash ^ u64::from(*id));
                for _ in 0..SECTION_VOLUME {
                    hash = mix(hash ^ u64::from(*id));
                }
            }
            Self::Paletted { palette, .. } => {
                let mut sorted = palette.clone();
                sorted.sort_unstable();
                for id in &sorted {
                    hash = mix(hash ^ u64::from(*id));
                }
                for index in 0..SECTION_VOLUME {
                    hash = mix(hash ^ u64::from(self.get_index(index)));
                }
            }
        }
        hash
    }

    /// The palette as an owned copy, for the save layer.
    #[must_use]
    pub fn palette_snapshot(&self) -> Vec<BlockId> {
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
    /// inside the palette. The result is never demoted; call [`Self::compact`].
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

/// Magic at the start of every saved section record.
pub const SECTION_RECORD_MAGIC: [u8; 4] = *b"TSEC";

/// Version of the canonical section record this build writes and reads.
///
/// Version 1 was the provisional layout in `terrain-gen`; the canonical record
/// stores names and a 32-byte baseline, so it is version 2.
pub const SECTION_RECORD_VERSION: u16 = 2;

/// Fixed bytes before the record body.
const RECORD_HEADER_LEN: usize = 53;

/// `edited` bit in the flag byte.
const FLAG_EDITED: u8 = 1;

/// Record kind for [`Section::Uniform`].
const KIND_UNIFORM: u8 = 0;

/// Record kind for [`Section::Paletted`].
const KIND_PALETTED: u8 = 1;

/// Most block types one section can hold (`I1.1`).
const MAX_PALETTE_ENTRIES: usize = 1 << 16;

/// Names every pack must hold to read saved sections (`§2.6`).
const CORE_NAMES: [&str; 3] = ["core:air", "core:bedrock", "core:water"];

/// The persisted identity of one saved section (`§3.2`).
///
/// This is the on-disk key: a column and a section slot. The revision carried by
/// [`crate::revisions::SectionKey`] is runtime state and is never saved, so the
/// save layer uses this smaller type.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SectionKey {
    /// The column, `[x, z]`.
    pub column: [i32; 2],
    /// Section slot in the column, `0..12`.
    pub section: u8,
}

/// One decoded saved-section record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedSection {
    /// Column and section slot the record names.
    pub key: SectionKey,
    /// Whether the section differs from its generated baseline (`J4.1`).
    pub edited: bool,
    /// Digest of the decoded ids as first generated (`J4.1`).
    pub baseline_hash: [u8; 32],
    /// The decoded section.
    pub section: Section,
    /// Stable names the pack did not hold, in record palette order.
    ///
    /// Each one resolved to a placeholder id; the record is still usable.
    pub unknown_names: Vec<String>,
}

/// The saved `baseline_hash` (`J4.1`): `SHA-256` over the decoded block ids of
/// one section, two little-endian bytes per cell, in [`section_index`] order.
///
/// This is the one pinned derivation. The save writer computes it over the
/// section as first generated and records it in the v2 record; `terrain-gen
/// verify` recomputes it over the decoded regenerated section and compares the
/// two. It is deliberately **not** [`Section::content_hash`]: that is a 64-bit
/// content digest with a different domain and does not fit the record's
/// 32-byte field.
#[must_use]
pub fn baseline_hash(section: &Section) -> [u8; 32] {
    let mut bytes = Vec::with_capacity(SECTION_VOLUME * 2);
    for index in 0..SECTION_VOLUME {
        bytes.extend_from_slice(&section.get_index(index).to_le_bytes());
    }
    crate::hash::sha256(&bytes)
}

/// Encodes one section record, checksum last (`J4.4`).
///
/// The palette is written in name-sorted order with its index lanes repacked to
/// match, so the bytes are canonical for the section's content. Runtime ids are
/// resolved to stable names through `pack`; an id the pack does not hold (only
/// possible for a section decoded with placeholders) is written as
/// `?unknown:<id>`, which is not a legal pack name. Always deterministic for the
/// same `(pack, key, baseline_hash, edited, section)`.
#[must_use]
pub fn encode_record(
    pack: &ContentPack,
    key: SectionKey,
    baseline_hash: [u8; 32],
    edited: bool,
    section: &Section,
) -> Vec<u8> {
    let (kind, solid, body) = encode_body(pack, section);
    let mut out = Vec::with_capacity(RECORD_HEADER_LEN + body.len() + 4);
    out.extend_from_slice(&SECTION_RECORD_MAGIC);
    out.extend_from_slice(&SECTION_RECORD_VERSION.to_le_bytes());
    out.extend_from_slice(&key.column[0].to_le_bytes());
    out.extend_from_slice(&key.column[1].to_le_bytes());
    out.push(key.section);
    out.push(if edited { FLAG_EDITED } else { 0 });
    out.extend_from_slice(&baseline_hash);
    out.push(kind);
    out.extend_from_slice(&solid.to_le_bytes());
    out.extend_from_slice(&body);
    let checksum = crc32(&out);
    out.extend_from_slice(&checksum.to_le_bytes());
    out
}

/// Decodes one in-memory section record (`J4.4`).
///
/// Because no path is known, a corrupt record reports
/// [`ErrorKind::CorruptSection`] with an empty path. Use
/// [`decode_record_at`] when the bytes came from a file so the error names it.
///
/// # Errors
///
/// `CorruptSection` (`Framing`, `Length`, or `Checksum`) for a malformed
/// record, and `ManifestIncompatible` when the pack cannot read saved sections
/// (a core name is missing) or an unknown-name placeholder range is exhausted.
pub fn decode_record(pack: &ContentPack, bytes: &[u8]) -> Result<SavedSection, TerrainError> {
    decode_bytes(pack, bytes, None)
}

/// Decodes one section record read from `path`, naming it in errors.
///
/// # Errors
///
/// The same kinds as [`decode_record`], with `path` attached to a
/// `CorruptSection`.
pub fn decode_record_at(
    pack: &ContentPack,
    path: &Path,
    bytes: &[u8],
) -> Result<SavedSection, TerrainError> {
    decode_bytes(pack, bytes, Some(path))
}

/// Builds the `(kind, solid, body)` triple for one section.
fn encode_body(pack: &ContentPack, section: &Section) -> (u8, u32, Vec<u8>) {
    match section {
        Section::Uniform(id) => {
            let solid = if *id == 0 { 0 } else { volume_u32() };
            (KIND_UNIFORM, solid, name_body(pack, *id))
        }
        Section::Paletted {
            palette,
            bits,
            data,
            ..
        } => {
            if palette.is_empty() {
                // A malformed in-memory palette has no representation; write
                // the empty section it semantically is instead of panicking.
                return (KIND_UNIFORM, 0, name_body(pack, 0));
            }
            encode_paletted(pack, palette, *bits, data)
        }
    }
}

/// Encodes a paletted body with a name-sorted palette and remapped lanes.
fn encode_paletted(
    pack: &ContentPack,
    palette: &[BlockId],
    bits: u8,
    data: &[u64],
) -> (u8, u32, Vec<u8>) {
    let names: Vec<String> = palette.iter().map(|id| block_name(pack, *id)).collect();
    let mut order: Vec<usize> = (0..palette.len()).collect();
    order.sort_by(|left, right| names[*left].cmp(&names[*right]));

    let mut new_slot = vec![0_usize; palette.len()];
    for (slot, &old) in order.iter().enumerate() {
        new_slot[old] = slot;
    }

    let out_bits = bits_for(palette.len());
    // A section is valid by construction, but the variant fields are public, so
    // a malformed width or short word slice degrades to lane 0 rather than
    // indexing out of bounds.
    let readable = matches!(bits, 1 | 2 | 4 | 8 | 16) && data.len() >= word_count(bits);
    let mut words = vec![0_u64; word_count(out_bits)].into_boxed_slice();
    let mut solid = 0_u32;
    for index in 0..SECTION_VOLUME {
        let old = if readable {
            read_word(data, index, bits)
        } else {
            0
        };
        let id = palette.get(old).copied().unwrap_or(0);
        solid += u32::from(id != 0);
        let slot = new_slot.get(old).copied().unwrap_or(0);
        write_word(&mut words, index, out_bits, slot);
    }

    let mut body = Vec::with_capacity(1 + 4 + palette.len() * 12 + words.len() * 8);
    body.push(out_bits);
    body.extend_from_slice(
        &u32::try_from(palette.len())
            .unwrap_or(u32::MAX)
            .to_le_bytes(),
    );
    for &slot in &order {
        push_name(&mut body, &names[slot]);
    }
    for word in &words {
        body.extend_from_slice(&word.to_le_bytes());
    }
    (KIND_PALETTED, solid, body)
}

/// The body of a uniform record: one name.
fn name_body(pack: &ContentPack, id: BlockId) -> Vec<u8> {
    let mut body = Vec::with_capacity(MAX_NAME_BYTES + 2);
    push_name(&mut body, &block_name(pack, id));
    body
}

/// The stable name of an id, or a non-pack diagnostic for a placeholder.
fn block_name(pack: &ContentPack, id: BlockId) -> String {
    pack.props(id)
        .map_or_else(|| format!("?unknown:{id}"), |record| record.name.clone())
}

/// Appends a `u16`-length-prefixed name.
fn push_name(out: &mut Vec<u8>, name: &str) {
    let bytes = name.as_bytes();
    let length = bytes.len().min(usize::from(u16::MAX));
    out.extend_from_slice(&u16::try_from(length).unwrap_or(u16::MAX).to_le_bytes());
    out.extend_from_slice(&bytes[..length]);
}

/// The shared decode path; `path` is `None` for an in-memory record.
fn decode_bytes(
    pack: &ContentPack,
    bytes: &[u8],
    path: Option<&Path>,
) -> Result<SavedSection, TerrainError> {
    let fail = |fault| corrupt(path, fault);
    let Some(magic) = bytes.get(..SECTION_RECORD_MAGIC.len()) else {
        return Err(fail(SectionFault::Length));
    };
    if magic != SECTION_RECORD_MAGIC {
        return Err(fail(SectionFault::Framing));
    }
    let Some(version) = bytes.get(4..6) else {
        return Err(fail(SectionFault::Length));
    };
    let version = u16::from_le_bytes([version[0], version[1]]);
    if version != SECTION_RECORD_VERSION {
        return Err(fail(SectionFault::Framing));
    }
    if bytes.len() < RECORD_HEADER_LEN + 4 {
        return Err(fail(SectionFault::Length));
    }

    // The checksum is verified before any field is trusted (`§16.4`).
    let body_end = bytes.len() - 4;
    let stored = u32::from_le_bytes(
        bytes[body_end..]
            .try_into()
            .map_err(|_| fail(SectionFault::Length))?,
    );
    if crc32(&bytes[..body_end]) != stored {
        return Err(fail(SectionFault::Checksum));
    }

    let mut reader = Reader::new(&bytes[..body_end]);
    reader.take(SECTION_RECORD_MAGIC.len() + 2).map_err(fail)?;
    let col_x = reader.i32().map_err(fail)?;
    let col_z = reader.i32().map_err(fail)?;
    let sy = reader.u8().map_err(fail)?;
    let flags = reader.u8().map_err(fail)?;
    let baseline_hash = reader.array().map_err(fail)?;
    let kind = reader.u8().map_err(fail)?;
    let solid = reader.u32().map_err(fail)?;

    if flags & !FLAG_EDITED != 0 {
        return Err(fail(SectionFault::Framing));
    }
    if u32::from(sy) >= SECTIONS_PER_COLUMN {
        return Err(fail(SectionFault::Framing));
    }
    require_core(pack)?;

    let mut unknowns = Unknowns::default();
    let section = match kind {
        KIND_UNIFORM => {
            let name = read_name(&mut reader).map_err(fail)?;
            let id = unknowns.resolve(pack, name)?;
            let expected = if id == 0 { 0 } else { volume_u32() };
            if solid != expected {
                return Err(fail(SectionFault::Framing));
            }
            Section::uniform(id)
        }
        KIND_PALETTED => {
            let bits = reader.u8().map_err(fail)?;
            let count = reader.u32().map_err(fail)?;
            let count = usize::try_from(count).map_err(|_| fail(SectionFault::Framing))?;
            if count == 0 || count > MAX_PALETTE_ENTRIES {
                return Err(fail(SectionFault::Framing));
            }
            if !matches!(bits, 1 | 2 | 4 | 8 | 16) || bits != bits_for(count) {
                return Err(fail(SectionFault::Framing));
            }
            let mut palette = Vec::with_capacity(count);
            for _ in 0..count {
                let name = read_name(&mut reader).map_err(fail)?;
                palette.push(unknowns.resolve(pack, name)?);
            }
            let mut data = Vec::with_capacity(word_count(bits));
            for _ in 0..word_count(bits) {
                data.push(reader.u64().map_err(fail)?);
            }
            let section = Section::from_palette(&palette, bits, &data)
                .map_err(|_| fail(SectionFault::Framing))?;
            let decoded =
                u32::try_from(section.solid_count()).map_err(|_| fail(SectionFault::Framing))?;
            if decoded != solid {
                return Err(fail(SectionFault::Framing));
            }
            section
        }
        _ => return Err(fail(SectionFault::Framing)),
    };

    if reader.at != body_end {
        return Err(fail(SectionFault::Length));
    }

    Ok(SavedSection {
        key: SectionKey {
            column: [col_x, col_z],
            section: sy,
        },
        edited: flags & FLAG_EDITED != 0,
        baseline_hash,
        section,
        unknown_names: unknowns.names,
    })
}

/// Checks the three names `§2.6` requires before any record is trusted.
fn require_core(pack: &ContentPack) -> Result<(), TerrainError> {
    for name in CORE_NAMES {
        if pack.id(name).is_none() {
            return Err(manifest_error(name, "missing core block name"));
        }
    }
    Ok(())
}

/// Placeholder ids for names the pack does not hold, first-seen order.
#[derive(Debug, Default)]
struct Unknowns {
    ids: BTreeMap<String, BlockId>,
    names: Vec<String>,
}

impl Unknowns {
    /// Resolves a saved name, allocating a placeholder for an unknown one.
    fn resolve(&mut self, pack: &ContentPack, name: &str) -> Result<BlockId, TerrainError> {
        if let Some(id) = pack.id(name) {
            return Ok(id);
        }
        if let Some(id) = self.ids.get(name) {
            return Ok(*id);
        }
        let next = u16::try_from(self.ids.len())
            .ok()
            .and_then(|index| UNKNOWN_BASE.checked_add(index));
        let Some(id) = next else {
            return Err(manifest_error(name, "unknown block name range exhausted"));
        };
        self.ids.insert(name.to_owned(), id);
        self.names.push(name.to_owned());
        Ok(id)
    }
}

/// Reads one length-prefixed stable name.
fn read_name<'a>(reader: &mut Reader<'a>) -> Result<&'a str, SectionFault> {
    let length = usize::from(reader.u16()?);
    if length == 0 || length > MAX_NAME_BYTES {
        return Err(SectionFault::Framing);
    }
    let raw = reader.take(length)?;
    std::str::from_utf8(raw).map_err(|_| SectionFault::Framing)
}

/// A bounds-checked reader over record bytes.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], SectionFault> {
        let end = self.at.checked_add(count).ok_or(SectionFault::Length)?;
        let slice = self.bytes.get(self.at..end).ok_or(SectionFault::Length)?;
        self.at = end;
        Ok(slice)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], SectionFault> {
        self.take(N)?.try_into().map_err(|_| SectionFault::Framing)
    }

    fn u8(&mut self) -> Result<u8, SectionFault> {
        self.take(1)?.first().copied().ok_or(SectionFault::Length)
    }

    fn u16(&mut self) -> Result<u16, SectionFault> {
        self.array().map(u16::from_le_bytes)
    }

    fn u32(&mut self) -> Result<u32, SectionFault> {
        self.array().map(u32::from_le_bytes)
    }

    fn i32(&mut self) -> Result<i32, SectionFault> {
        self.array().map(i32::from_le_bytes)
    }

    fn u64(&mut self) -> Result<u64, SectionFault> {
        self.array().map(u64::from_le_bytes)
    }
}

/// The record error for `path`, or an empty path for an in-memory record.
fn corrupt(path: Option<&Path>, fault: SectionFault) -> TerrainError {
    TerrainError::new(
        Stage::Persistence,
        ErrorKind::CorruptSection {
            path: path.map_or_else(PathBuf::new, Path::to_path_buf),
            fault,
        },
    )
}

/// A pack that cannot read saved sections is a manifest failure, not corruption.
fn manifest_error(id: &str, reason: &'static str) -> TerrainError {
    TerrainError::new(
        Stage::Persistence,
        ErrorKind::ManifestIncompatible {
            id: id.to_owned(),
            reason,
        },
    )
}

/// Index of a local cell in a section.
///
/// The only place the formula exists. Coordinates are masked to `0..32`, so a
/// caller may pass a world coordinate and address `(x & 31, y & 31, z & 31)`.
#[must_use]
pub fn section_index(x: u32, y: u32, z: u32) -> usize {
    ((x & LOCAL_MASK) | ((z & LOCAL_MASK) << LOCAL_SHIFT) | ((y & LOCAL_MASK) << (2 * LOCAL_SHIFT)))
        as usize
}

/// The `(x, y, z)` local cell of a section index.
///
/// The inverse of [`section_index`]. Indices are masked to the volume.
#[must_use]
pub fn section_coord(index: usize) -> (u32, u32, u32) {
    let index = u32::try_from(index & (SECTION_VOLUME - 1)).expect("a masked index fits u32");
    let x = index & LOCAL_MASK;
    let z = (index >> LOCAL_SHIFT) & LOCAL_MASK;
    let y = (index >> (2 * LOCAL_SHIFT)) & LOCAL_MASK;
    (x, y, z)
}

/// Smallest allowed lane width that holds `entries` palette entries.
///
/// A section has at most 65,536 block types, so `16` is the ceiling.
const fn bits_for(entries: usize) -> u8 {
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
const fn lanes_per_word(bits: u8) -> usize {
    (64 / bits) as usize
}

/// Words a packed section needs at `bits` per lane.
const fn word_count(bits: u8) -> usize {
    SECTION_VOLUME.div_ceil(lanes_per_word(bits))
}

/// The lane value mask for `bits` per lane.
const fn mask_for(bits: u8) -> u64 {
    (1_u64 << bits) - 1
}

/// Word and bit offset of one lane. Every lane sits wholly inside one word.
const fn lane(index: usize, bits: u8) -> (usize, usize) {
    let per_word = lanes_per_word(bits);
    (index / per_word, (index % per_word) * (bits as usize))
}

/// Reads one packed lane.
fn read_word(data: &[u64], index: usize, bits: u8) -> usize {
    let (word, shift) = lane(index, bits);
    usize::try_from((data[word] >> shift) & mask_for(bits)).expect("a lane fits usize")
}

/// Writes one packed lane, leaving every other bit of its word untouched.
fn write_word(data: &mut [u64], index: usize, bits: u8, value: usize) {
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
fn volume_u32() -> u32 {
    u32::try_from(SECTION_VOLUME).expect("a section volume fits u32")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::{domain, hash64};

    /// The smallest palette that forces each width.
    const WIDTHS: [(u8, usize); 5] = [(1, 2), (2, 3), (4, 5), (8, 17), (16, 257)];

    /// A heap-backed cell array, because the API takes `&[BlockId; SECTION_VOLUME]`.
    fn scratch_blocks() -> Box<[BlockId; SECTION_VOLUME]> {
        vec![0_u16; SECTION_VOLUME]
            .into_boxed_slice()
            .try_into()
            .expect("the scratch array matches the section volume")
    }

    fn palette_of(entries: usize) -> Vec<BlockId> {
        (0..entries)
            .map(|slot| u16::try_from(slot + 1).expect("a palette id fits u16"))
            .collect()
    }

    fn boundary_cells(per_word: usize) -> Vec<usize> {
        let middle = (SECTION_VOLUME / 2 / per_word) * per_word;
        let last = SECTION_VOLUME - per_word;
        let mut cells = vec![
            0,
            per_word - 1,
            per_word,
            2 * per_word - 1,
            middle - 1,
            middle,
            middle + per_word - 1,
            last,
            last + per_word - 1,
        ];
        cells.sort_unstable();
        cells.dedup();
        cells
    }

    #[test]
    fn index_and_inverse_round_trip() {
        for index in 0..SECTION_VOLUME {
            let (x, y, z) = section_coord(index);
            assert!(x < SECTION_SIZE && y < SECTION_SIZE && z < SECTION_SIZE);
            assert_eq!(section_index(x, y, z), index);
            let recomputed = usize::try_from(x + 32 * (z + 32 * y)).expect("index fits usize");
            assert_eq!(section_index(x, y, z), recomputed);
        }

        assert_eq!(section_index(SECTION_SIZE, 0, 0), section_index(0, 0, 0));
        assert_eq!(
            section_index(0, 0, SECTION_SIZE + 7),
            section_index(0, 0, 7)
        );
        assert_eq!(
            section_index(0, SECTION_SIZE + 3, 0),
            section_index(0, 3, 0)
        );
    }

    #[test]
    fn pack_unpack_at_every_bit_width() {
        let tag = domain("section:test");
        for (bits, entries) in WIDTHS {
            let palette = palette_of(entries);
            let empty = vec![0_u64; word_count(bits)];
            let mut section = Section::from_palette(&palette, bits, &empty).expect("valid palette");
            assert_eq!(section.bits(), bits);

            let mut expected = vec![palette[0]; SECTION_VOLUME];
            for (index, cell) in expected.iter_mut().enumerate() {
                let hash = hash64(1, tag, &[i64::try_from(index).expect("index fits")]);
                let slot = usize::try_from(hash % u64::try_from(entries).expect("entries fit"))
                    .expect("slot fits usize");
                *cell = palette[slot];
                section.set_index(index, palette[slot]);
            }

            assert_eq!(section.bits(), bits, "no growth for a full palette");
            let mut out = scratch_blocks();
            section.write_blocks(&mut out);
            assert_eq!(out.as_slice(), expected.as_slice(), "bits {bits}");
            for (index, id) in expected.iter().enumerate() {
                assert_eq!(section.get_index(index), *id, "bits {bits} index {index}");
            }
        }
    }

    #[test]
    fn palette_growth_repacks() {
        let ids: Vec<BlockId> = (0..300_u16).map(|i| i * 7 + 3).collect();
        let mut section = Section::uniform(ids[0]);
        let mut expected = vec![ids[0]; SECTION_VOLUME];
        let mut assigned = 0;

        for (count, width) in [(1_usize, 1_u8), (3, 2), (9, 4), (17, 8), (300, 16)] {
            while assigned < count {
                // 101 is coprime with the volume, so the first 300 cells differ.
                let cell = (assigned * 101 + 7) % SECTION_VOLUME;
                section.set_index(cell, ids[assigned]);
                expected[cell] = ids[assigned];
                assigned += 1;
            }
            assert_eq!(section.bits(), width, "count {count}");
            assert_eq!(section.palette().len(), count, "count {count}");
            let mut out = scratch_blocks();
            section.write_blocks(&mut out);
            assert_eq!(out.as_slice(), expected.as_slice(), "count {count}");
        }
    }

    #[test]
    fn uniform_promotes_and_demotes() {
        let mut section = Section::uniform(9);
        assert_eq!(section.is_uniform(), Some(9));
        assert_eq!(section.solid_count(), SECTION_VOLUME);
        assert!(matches!(section, Section::Uniform(9)));

        section.set(4, 5, 6, 12);
        assert!(matches!(section, Section::Paletted { .. }));
        assert_eq!(section.is_uniform(), None);
        assert_eq!(section.get(4, 5, 6), 12);
        assert_eq!(section.get(4, 5, 7), 9);
        assert_eq!(section.bits(), 1);

        section.set(4, 5, 6, 9);
        assert!(
            matches!(section, Section::Paletted { .. }),
            "a write never demotes implicitly"
        );
        assert_eq!(section.solid_count(), SECTION_VOLUME);

        section.compact();
        assert!(matches!(section, Section::Uniform(9)));
        assert_eq!(section.is_uniform(), Some(9));
        assert_eq!(section.bits(), 1);
    }

    #[test]
    fn unknown_name_round_trip() {
        let name: BlockId = 60_000;
        let mut blocks = scratch_blocks();
        blocks.fill(name);
        blocks[0] = 0;
        blocks[17] = 12_345;

        let mut section = Section::from_blocks(&blocks);
        assert!(matches!(section, Section::Paletted { .. }));
        assert_eq!(section.get_index(0), 0);
        assert_eq!(section.get_index(17), 12_345);
        assert_eq!(section.get(1, 2, 3), name);

        section.compact();
        assert!(
            matches!(section, Section::Paletted { .. }),
            "two ids stay paletted"
        );
        section.set_index(SECTION_VOLUME - 1, name);
        assert_eq!(section.get(31, 31, 31), name);

        let mut out = scratch_blocks();
        section.write_blocks(&mut out);
        assert_eq!(out.as_slice(), blocks.as_slice());

        let rebuilt = Section::from_blocks(&blocks);
        assert_eq!(rebuilt.content_hash(), section.content_hash());
        assert_eq!(rebuilt, section);
        assert!(section.palette_snapshot().contains(&name));
    }

    #[test]
    fn content_hash_ignores_palette_order() {
        let mut blocks = scratch_blocks();
        blocks.fill(11);
        blocks[1] = 22;
        blocks[2] = 33;

        let sorted = Section::from_blocks(&blocks);
        let mut incremental = Section::uniform(11);
        for (index, id) in blocks.iter().enumerate().rev() {
            incremental.set_index(index, *id);
        }

        assert_ne!(
            sorted.palette_snapshot(),
            incremental.palette_snapshot(),
            "the test needs two different palette orders"
        );
        assert_eq!(sorted, incremental);
        assert_eq!(sorted.content_hash(), incremental.content_hash());

        let mut other = blocks;
        other[3] = 44;
        assert_ne!(
            sorted.content_hash(),
            Section::from_blocks(&other).content_hash()
        );
    }

    #[test]
    fn solid_count_is_o1_and_correct() {
        let mut section = Section::uniform(0);
        assert_eq!(section.solid_count(), 0);
        assert!(section.is_empty(0));

        section = Section::uniform(5);
        assert_eq!(section.solid_count(), SECTION_VOLUME);
        assert!(!section.is_empty(0));

        let mut solid = SECTION_VOLUME;
        for step in 0..1000_usize {
            let index = (step * 37) % SECTION_VOLUME;
            let id = u16::try_from(step % 6).expect("id fits u16");
            let previous = section.get_index(index);
            section.set_index(index, id);
            if previous != id {
                solid -= usize::from(previous != 0);
                solid += usize::from(id != 0);
            }
            assert_eq!(section.solid_count(), solid, "step {step}");
        }

        let mut cleared = Section::uniform(3);
        for index in 0..64 {
            cleared.set_index(index, 4);
        }
        for index in 0..SECTION_VOLUME {
            cleared.set_index(index, 0);
        }
        assert_eq!(cleared.solid_count(), 0);
        assert!(
            cleared.is_empty(0),
            "stale palette entries do not keep a section non-empty"
        );

        let air = 7_u16;
        let mut uniform_air = Section::uniform(air);
        assert!(uniform_air.is_empty(air));
        assert!(!uniform_air.is_empty(0));
        uniform_air.set(1, 1, 1, 8);
        assert!(!uniform_air.is_empty(air));
    }

    #[test]
    fn words_never_straddle() {
        for (bits, entries) in WIDTHS {
            let palette = palette_of(entries);
            let empty = vec![0_u64; word_count(bits)];
            let mut section = Section::from_palette(&palette, bits, &empty).expect("valid palette");

            for index in 0..SECTION_VOLUME {
                section.set_index(index, palette[index % entries]);
            }

            let per_word = lanes_per_word(bits);
            for index in 0..SECTION_VOLUME {
                let (word, shift) = lane(index, bits);
                assert!(shift + usize::from(bits) <= 64, "bits {bits} index {index}");
                let stored = usize::try_from((section.words()[word] >> shift) & mask_for(bits))
                    .expect("lane fits usize");
                assert_eq!(stored, index % entries, "bits {bits} index {index}");
            }

            for index in boundary_cells(per_word) {
                let (word, shift) = lane(index, bits);
                let mask = mask_for(bits) << shift;
                let before = section.words().to_vec();
                let next = (read_word(&before, index, bits) + 1) % entries;
                section.set_index(index, palette[next]);

                for (position, (old, new)) in before.iter().zip(section.words()).enumerate() {
                    if position == word {
                        assert_eq!(
                            (old ^ new) & !mask,
                            0,
                            "bits {bits} index {index} wrote outside its lane"
                        );
                        assert_ne!(old, new, "bits {bits} index {index} did not change");
                    } else {
                        assert_eq!(
                            old, new,
                            "bits {bits} index {index} touched word {position}"
                        );
                    }
                }
            }
        }
    }

    /// A pack with `extra` synthetic `test:bNNN` records for wide palettes.
    fn test_pack(extra: usize) -> ContentPack {
        let mut records = crate::content::v1_default().records().to_vec();
        let template = records[0].clone();
        for slot in 0..extra {
            let mut record = template.clone();
            record.name = format!("test:b{slot:03}");
            records.push(record);
        }
        ContentPack::load(records).expect("the test pack is valid")
    }

    /// The first `entries` names of `pack`, as ids.
    fn palette_ids(pack: &ContentPack, entries: usize) -> Vec<BlockId> {
        pack.records()[..entries]
            .iter()
            .map(|record| pack.id(&record.name).expect("every record has an id"))
            .collect()
    }

    /// A paletted section holding `entries` distinct blocks at every cell.
    fn paletted(pack: &ContentPack, entries: usize) -> Section {
        let palette = palette_ids(pack, entries);
        let bits = bits_for(entries);
        let mut data = vec![0_u64; word_count(bits)].into_boxed_slice();
        for index in 0..SECTION_VOLUME {
            let slot = (index.wrapping_mul(37) + index / 11) % entries;
            write_word(&mut data, index, bits, slot);
        }
        Section::from_palette(&palette, bits, &data).expect("a valid paletted section")
    }

    fn record_key() -> SectionKey {
        SectionKey {
            column: [3, -5],
            section: 7,
        }
    }

    const BASELINE: [u8; 32] = [0xAB; 32];

    fn fault(error: &TerrainError) -> SectionFault {
        match &error.kind {
            ErrorKind::CorruptSection { fault, .. } => *fault,
            other => panic!("expected CorruptSection, found {other:?}"),
        }
    }

    /// Recomputes the trailing checksum after a deliberate field edit.
    fn reseal(mut bytes: Vec<u8>) -> Vec<u8> {
        let body_end = bytes.len() - 4;
        let checksum = crc32(&bytes[..body_end]);
        bytes[body_end..].copy_from_slice(&checksum.to_le_bytes());
        bytes
    }

    fn to_hex(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            let _ = std::fmt::Write::write_fmt(&mut out, format_args!("{byte:02x}"));
        }
        out
    }

    /// A paletted section with the palette reversed and its lanes remapped.
    fn reversed_palette(section: &Section) -> Section {
        let Section::Paletted {
            palette,
            bits,
            data,
            ..
        } = section
        else {
            panic!("the test needs a paletted section");
        };
        let reversed: Vec<BlockId> = palette.iter().rev().copied().collect();
        let last = palette.len() - 1;
        let mut words = vec![0_u64; word_count(*bits)].into_boxed_slice();
        for index in 0..SECTION_VOLUME {
            let old = read_word(data, index, *bits);
            write_word(&mut words, index, *bits, last - old);
        }
        Section::from_palette(&reversed, *bits, &words).expect("a valid reversed section")
    }

    #[test]
    fn saved_record_round_trips_at_every_bit_width() {
        let pack = test_pack(300);
        for (bits, entries) in WIDTHS {
            let section = paletted(&pack, entries);
            assert_eq!(bits_for(entries), bits, "the test covers every width");
            let bytes = encode_record(&pack, record_key(), BASELINE, false, &section);
            let saved =
                decode_record(&pack, &bytes).unwrap_or_else(|error| panic!("bits {bits}: {error}"));
            assert_eq!(saved.key, record_key(), "bits {bits}");
            assert!(!saved.edited, "bits {bits}");
            assert_eq!(saved.baseline_hash, BASELINE, "bits {bits}");
            assert_eq!(saved.section, section, "bits {bits}");
            assert!(saved.unknown_names.is_empty(), "bits {bits}");
        }
    }

    #[test]
    fn one_entry_palette_round_trips() {
        let pack = crate::content::v1_default();
        let stone = pack.id("core:stone").expect("core:stone is in the pack");
        let data = vec![0_u64; word_count(1)];
        let section =
            Section::from_palette(&[stone], 1, &data).expect("a one-entry palette is valid");

        let bytes = encode_record(&pack, record_key(), BASELINE, false, &section);
        let saved = decode_record(&pack, &bytes).expect("the record decodes");
        assert_eq!(saved.section, section);
        assert!(
            matches!(saved.section, Section::Paletted { .. }),
            "the record keeps the one-entry palette paletted"
        );
    }

    #[test]
    fn uniform_round_trips_and_stays_small() {
        let pack = crate::content::v1_default();
        let stone = pack.id("core:stone").expect("core:stone is in the pack");
        for (id, expected_solid) in [(0_u16, 0_u32), (stone, 32_768)] {
            let section = Section::uniform(id);
            let bytes = encode_record(&pack, record_key(), BASELINE, false, &section);
            let name = pack.props(id).expect("the id resolves").name.as_str();
            assert_eq!(
                bytes.len(),
                RECORD_HEADER_LEN + 2 + name.len() + 4,
                "a uniform record carries no index payload"
            );
            let saved = decode_record(&pack, &bytes).expect("the record decodes");
            assert_eq!(saved.section, section);
            assert_eq!(saved.section.is_uniform(), Some(id));
            assert_eq!(
                saved.section.solid_count(),
                usize::try_from(expected_solid).expect("a solid count fits usize")
            );
        }
    }

    #[test]
    fn unknown_names_degrade_and_are_reported() {
        let mut records = crate::content::v1_default().records().to_vec();
        let template = records[0].clone();
        let mut gone = template.clone();
        gone.name = "mod:gone".to_owned();
        let mut also = template;
        also.name = "mod:also".to_owned();
        records.push(gone);
        records.push(also);
        let source = ContentPack::load(records).expect("the extended pack is valid");
        let pack = crate::content::v1_default();

        let gone_id = source.id("mod:gone").expect("the extra name resolves");
        let also_id = source.id("mod:also").expect("the extra name resolves");
        let mut blocks = scratch_blocks();
        blocks.fill(0);
        blocks[0] = gone_id;
        blocks[7] = also_id;
        let section = Section::from_blocks(&blocks);
        let bytes = encode_record(&source, record_key(), BASELINE, false, &section);
        let saved = decode_record(&pack, &bytes).expect("an unknown name is not corruption");

        assert_eq!(saved.unknown_names, ["mod:also", "mod:gone"]);
        assert_eq!(saved.section.get_index(0), UNKNOWN_BASE + 1);
        assert_eq!(saved.section.get_index(7), UNKNOWN_BASE);
        assert_eq!(saved.section.get_index(1), 0);

        // An id outside the pack has no stable name; encode marks it so a
        // reader reports it instead of inventing content.
        let placeholder = Section::uniform(UNKNOWN_BASE);
        let bytes = encode_record(&pack, record_key(), BASELINE, false, &placeholder);
        let saved = decode_record(&pack, &bytes).expect("a placeholder decodes as unknown");
        assert_eq!(saved.section.is_uniform(), Some(UNKNOWN_BASE));
        assert_eq!(saved.unknown_names, ["?unknown:61440"]);
    }

    #[test]
    fn truncation_is_a_hard_error_at_every_length() {
        let pack = test_pack(0);
        let section = paletted(&pack, 3);
        let bytes = encode_record(&pack, record_key(), BASELINE, false, &section);
        for length in 0..bytes.len() {
            let error = decode_record(&pack, &bytes[..length]).unwrap_err();
            let fault = fault(&error);
            assert!(
                matches!(fault, SectionFault::Length | SectionFault::Checksum),
                "a {length}-byte record reported {fault:?}"
            );
            if length < RECORD_HEADER_LEN + 4 {
                assert_eq!(
                    fault,
                    SectionFault::Length,
                    "a short record is a length fault"
                );
            }
        }
    }

    #[test]
    fn checksum_covers_payload_and_crc() {
        let pack = test_pack(0);
        let section = paletted(&pack, 3);
        let bytes = encode_record(&pack, record_key(), BASELINE, false, &section);

        let mut payload = bytes.clone();
        payload[RECORD_HEADER_LEN + 1] ^= 0x01;
        assert_eq!(
            fault(&decode_record(&pack, &payload).unwrap_err()),
            SectionFault::Checksum
        );

        let mut crc = bytes;
        let last = crc.len() - 1;
        crc[last] ^= 0x01;
        assert_eq!(
            fault(&decode_record(&pack, &crc).unwrap_err()),
            SectionFault::Checksum
        );
    }

    #[test]
    fn impossible_bit_widths_are_framing_faults() {
        let pack = test_pack(0);
        let section = paletted(&pack, 3);
        assert_eq!(bits_for(3), 2);
        let bytes = encode_record(&pack, record_key(), BASELINE, false, &section);

        for bad in [0_u8, 1, 3, 5, 9, 32] {
            let mut mutated = bytes.clone();
            mutated[RECORD_HEADER_LEN] = bad;
            let mutated = reseal(mutated);
            assert_eq!(
                fault(&decode_record(&pack, &mutated).unwrap_err()),
                SectionFault::Framing,
                "bits {bad}"
            );
        }
    }

    #[test]
    fn an_out_of_range_index_is_a_framing_fault() {
        let pack = test_pack(0);
        let section = paletted(&pack, 3);
        let bytes = encode_record(&pack, record_key(), BASELINE, false, &section);
        let words_at = bytes.len() - 4 - word_count(2) * 8;

        let mut mutated = bytes;
        mutated[words_at] = 3;
        let mutated = reseal(mutated);
        assert_eq!(
            fault(&decode_record(&pack, &mutated).unwrap_err()),
            SectionFault::Framing
        );
    }

    #[test]
    fn edited_and_baseline_are_preserved() {
        let pack = crate::content::v1_default();
        let section = Section::uniform(0);
        for edited in [false, true] {
            let baseline = [u8::from(edited); 32];
            let bytes = encode_record(&pack, record_key(), baseline, edited, &section);
            let saved = decode_record(&pack, &bytes).expect("the record decodes");
            assert_eq!(saved.edited, edited);
            assert_eq!(saved.baseline_hash, baseline);
        }
    }

    #[test]
    fn encoding_is_deterministic_and_ignores_palette_order() {
        let pack = test_pack(0);
        let section = paletted(&pack, 5);
        let first = encode_record(&pack, record_key(), BASELINE, true, &section);
        let second = encode_record(&pack, record_key(), BASELINE, true, &section);
        assert_eq!(first, second, "two encodes of one section are identical");

        let reversed = reversed_palette(&section);
        assert_ne!(
            section.palette_snapshot(),
            reversed.palette_snapshot(),
            "the test needs two different palette orders"
        );
        assert_eq!(section, reversed);
        assert_eq!(
            first,
            encode_record(&pack, record_key(), BASELINE, true, &reversed),
            "a name-sorted palette makes the bytes canonical"
        );
    }

    #[test]
    fn a_missing_core_name_is_a_manifest_failure() {
        let mut records = crate::content::v1_default().records().to_vec();
        records.retain(|record| record.name != "core:water");
        let pack = ContentPack::load(records).expect("the reduced pack is valid");
        let bytes = encode_record(
            &crate::content::v1_default(),
            record_key(),
            BASELINE,
            false,
            &Section::uniform(0),
        );
        let error = decode_record(&pack, &bytes).unwrap_err();
        assert!(
            matches!(&error.kind, ErrorKind::ManifestIncompatible { id, .. } if id == "core:water"),
            "found {error:?}"
        );
    }

    #[test]
    fn a_path_is_named_in_a_file_record_error() {
        let pack = crate::content::v1_default();
        let bytes = encode_record(&pack, record_key(), BASELINE, false, &Section::uniform(0));
        let mut corrupted = bytes;
        corrupted[RECORD_HEADER_LEN + 2] ^= 0x01;
        let path = Path::new("world/chunks/c.0.0.0.sect");
        let error = decode_record_at(&pack, path, &corrupted).unwrap_err();
        match &error.kind {
            ErrorKind::CorruptSection { path: named, fault } => {
                assert_eq!(named, path);
                assert_eq!(*fault, SectionFault::Checksum);
            }
            other => panic!("expected CorruptSection, found {other:?}"),
        }
    }

    #[test]
    fn uniform_record_bytes_are_golden() {
        let pack = crate::content::v1_default();
        let stone = pack.id("core:stone").expect("core:stone is in the pack");
        let bytes = encode_record(
            &pack,
            record_key(),
            BASELINE,
            true,
            &Section::uniform(stone),
        );
        assert_eq!(to_hex(&bytes), GOLDEN_UNIFORM_HEX);
    }

    #[test]
    fn paletted_record_prefix_is_golden() {
        let pack = crate::content::v1_default();
        let stone = pack.id("core:stone").expect("core:stone is in the pack");
        let air = pack.id("core:air").expect("core:air is in the pack");
        let mut data = vec![0_u64; word_count(1)].into_boxed_slice();
        write_word(&mut data, 0, 1, 1);
        let section =
            Section::from_palette(&[air, stone], 1, &data).expect("a two-entry palette is valid");

        let bytes = encode_record(&pack, record_key(), BASELINE, false, &section);
        assert_eq!(bytes.len(), RECORD_HEADER_LEN + 1 + 4 + 22 + 4096 + 4);
        assert_eq!(
            u64::from_le_bytes(
                bytes[RECORD_HEADER_LEN + 27..RECORD_HEADER_LEN + 35]
                    .try_into()
                    .expect("eight bytes")
            ),
            1,
            "cell zero holds palette slot one"
        );
        assert_eq!(
            to_hex(&bytes[..RECORD_HEADER_LEN + 35]),
            GOLDEN_PALETTED_PREFIX_HEX
        );
    }

    const GOLDEN_UNIFORM_HEX: &str = "54534543020003000000fbffffff0701abababababababababababababababababababababababababababababababab00008000000a00636f72653a73746f6e655fd7787b";
    const GOLDEN_PALETTED_PREFIX_HEX: &str = "54534543020003000000fbffffff0700abababababababababababababababababababababababababababababababab010100000001020000000800636f72653a6169720a00636f72653a73746f6e650100000000000000";
}
