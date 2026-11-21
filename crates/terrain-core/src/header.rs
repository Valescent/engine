//! `header.bin`: the mandatory world header and the load order (`A1.2`, `A1.5`).
//!
//! The header is the only file a loader reads before it trusts anything else. It
//! is fixed little-endian, versioned, and carries a trailing `crc32` over every
//! preceding byte, so a torn or edited header is detected before any field is
//! trusted. Nothing derived, runtime-only, or per-section lives here.
//!
//! # Layout (`A1.2`)
//!
//! | Offset | Width | Field |
//! |---|---|---|
//! | `0..4` | 4 | magic `"TWLD"` |
//! | `4..6` | 2 | `format_version: u16` |
//! | `6..8` | 2 | `generator_version: u16` |
//! | `8..40` | 32 | `description_hash: [u8; 32]` (SHA-256) |
//! | `40..41` | 1 | `hash_algorithm` byte length, then that many UTF-8 bytes |
//! | … | 8 | `seed: u64` |
//! | … | 1 + n | `platform_tag` byte length, then that many UTF-8 bytes |
//! | … | 1 + n | `last_saved_platform_tag` |
//! | … | 16 | `world_id: [u8; 16]` |
//! | … | 1 + n | `name` byte length, then that many UTF-8 bytes |
//! | … | 8 | `created: i64` (UTC Unix seconds) |
//! | … | 8 | `last_saved: i64` (UTC Unix seconds) |
//! | … | 8 | `revision: u64` |
//! | … | 2 | `schema_version: u16` |
//! | … | 4 | `crc32: u32` over every preceding byte |
//!
//! Strings are length-prefixed with one byte, which bounds each to 255 bytes;
//! [`Header::validate`] refuses to write a longer one rather than truncating.
//! `created` and `last_saved` are UTC epoch seconds, never ordering keys.
//!
//! # Durability (`J3.4`)
//!
//! [`Header::write`] stages a temp file, `fsync`s it, renames it over the
//! destination, then `fsync`s the containing directory. On Windows the
//! directory `fsync` is a no-op, because `std` cannot open a directory handle.
//!
//! # Load order (`A1.5`)
//!
//! [`load_order`] is the refusal order: header crc, format support, description
//! present and parses, recomputed hash matches, generator version supported,
//! manifest validation, journal recovery. [`refuse_reason`] names the step that
//! refused a given error. A missing description is an error; this module never
//! creates a world file.

mod codec;
mod layout;

pub use codec::{HASH_ALGORITHM, LoadStep, crc32, load_order, refuse_reason};
pub use layout::{
    HEADER_FILE, HEADER_FORMAT_V1, HEADER_FORMAT_VERSION, Header, LOAD_STEP_COUNT, MAGIC,
    MigrationState, now_unix_seconds,
};
