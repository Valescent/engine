//! `W1` section storage: a 32³ block array stored as `Uniform` or `Paletted`.
//!
//! `I1.1`, `I1.2`. A section is 32 blocks on every axis and the cell order is
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
//!
//! # Saved record (`J4.1`, `J4.2`, `J4.4`)
//!
//! [`encode_record`] and [`decode_record`] define the canonical on-disk section
//! payload. It stores **stable block names**, never runtime ids (`A5.1`,
//! `T1.6`): the encoder maps ids through a [`ContentPack`](crate::content::ContentPack)
//! and the decoder maps names back. All integers are little-endian.
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
//! `UNKNOWN_BASE` — one per distinct unknown
//! name, in palette order — and is reported in [`SavedSection::unknown_names`].
//! It never fails the record. The placeholder range is finite, so a section with
//! more distinct unknown names than it can hold is a `ManifestIncompatible`
//! manifest failure rather than a corrupt record. A core name missing from the
//! pack, a bad palette, an impossible bit width, an out-of-range index, a
//! truncated payload, or a bad checksum is a hard error.
//!
//! The implementation is split into `storage` (the section itself and index
//! mapping), `codec` (the saved record), and `baseline` (content and baseline
//! hashing). Every public item is re-exported here, so `section::<item>` paths
//! are unchanged.

mod baseline;
mod codec;
mod storage;

#[cfg(test)]
mod tests;

pub use baseline::baseline_hash;
#[allow(unused_imports)] // crate-private re-exports keep the flat module paths
pub(crate) use codec::{SECTION_RECORD_MAGIC, SECTION_RECORD_VERSION};
pub use codec::{SavedSection, decode_record, decode_record_at, encode_record};
pub use storage::{BlockId, Section, SectionKey};
#[allow(unused_imports)] // crate-private re-exports keep the flat module paths
pub(crate) use storage::{section_coord, section_index};
