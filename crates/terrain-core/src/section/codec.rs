//! The canonical `TSEC` saved-section record (`J4.1`, `J4.2`, `J4.4`).
//!
//! [`encode_record`] and [`decode_record`] define the canonical on-disk section
//! payload. It stores **stable block names**, never runtime ids (`A5.1`,
//! `T1.6`): the encoder maps ids through a [`ContentPack`] and the decoder maps
//! names back. All integers are little-endian.
//!
//! | Offset | Width | Field |
//! |---|---|---|
//! | `0..4` | 4 | magic `"TSEC"` |
//! | `4..6` | 2 | record version: `SECTION_RECORD_VERSION` |
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
//! `UNKNOWN_BASE` — one per distinct unknown name, in palette order — and is
//! reported in [`SavedSection::unknown_names`]. It never fails the record. The
//! placeholder range is finite, so a section with more distinct unknown names
//! than it can hold is a `ManifestIncompatible` manifest failure rather than a
//! corrupt record. A core name missing from the pack, a bad palette, an
//! impossible bit width, an out-of-range index, a truncated payload, or a bad
//! checksum is a hard error.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::storage::{
    Section, SectionKey, bits_for, read_word, volume_u32, word_count, write_word,
};
use crate::content::{BlockId, ContentPack, MAX_NAME_BYTES, UNKNOWN_BASE};
use crate::description::{SECTION_VOLUME, SECTIONS_PER_COLUMN};
use crate::error::{ErrorKind, SectionFault, Stage, TerrainError};
use crate::header::crc32;

/// Magic at the start of every saved section record.
pub(crate) const SECTION_RECORD_MAGIC: [u8; 4] = *b"TSEC";

/// Version of the canonical section record this build writes and reads.
///
/// Version 1 was the provisional layout in `terrain-gen`; the canonical record
/// stores names and a 32-byte baseline, so it is version 2.
pub(crate) const SECTION_RECORD_VERSION: u16 = 2;

/// Fixed bytes before the record body.
pub(super) const RECORD_HEADER_LEN: usize = 53;

/// `edited` bit in the flag byte.
const FLAG_EDITED: u8 = 1;

/// Record kind for `Section::Uniform`.
const KIND_UNIFORM: u8 = 0;

/// Record kind for [`Section::Paletted`].
const KIND_PALETTED: u8 = 1;

/// Most block types one section can hold (`I1.1`).
const MAX_PALETTE_ENTRIES: usize = 1 << 16;

/// Names every pack must hold to read saved sections (`§2.6`).
const CORE_NAMES: [&str; 3] = ["core:air", "core:bedrock", "core:water"];

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
