//! Content pack: stable names, runtime ids, per-record hashes (`A5`).
//!
//! `A5.1`–`A5.4`. The parsed pack is the **sole authority** for block
//! properties (`A5.2`): nothing downstream may carry a second copy of a
//! property or attach behaviour to a number. Code resolves a block once at
//! startup by name (`pack.id("core:torch")`) or by tag, never by comparing
//! strings in a hot path.
//!
//! # Identity
//!
//! Stable names are what a save file, an edit record, and a description pin.
//! [`BlockId`](crate::section::BlockId)s are **derived at load** by sorting records by name (`A5.1`), so
//! one pack yields the same ids on every run, and two packs that differ only in
//! declaration order produce identical ids. Ids are never saved, hashed, or
//! reported; they are an in-memory index into [`ContentPack::records`].
//!
//! [`record_hash`] covers canonical parsed bytes (`A5.3`): struct field order,
//! little-endian fixed-width integers, length-prefixed strings, sorted map
//! entries, and `-0.0` normalised to `+0.0`. Whitespace, comments, and file
//! order never enter it. [`ContentPack::pack_hash`] hashes the name-sorted
//! `(kind, name, record_version, record_hash)` list, which is what a
//! description pins through its `CONTENT` reference.
//!
//! # Placeholder pack
//!
//! `OPEN(content ids)` the real pack does not exist yet. [`v1_default`] is a
//! minimal placeholder for the thirteen block ids the pipeline emits today
//! (`core:air` … `core:plant`) plus the five material ids `§2.6` and `§8.3`
//! need: `core:bedrock`, `core:clay`, `core:gravel`, `core:ice`, and
//! `core:snow`. It exists so lighting, meshing, the column pipeline, and the
//! surface rules have properties to read; it is not an authored pack, and its
//! `hardness` and `drops` values are invented. Because `A5.1` derives ids by
//! name, its numbering is **not** the legacy numbering that `pipeline`,
//! `light2`, and `mesher2` still hard-code; those constants are the pre-pack
//! placeholders and must be replaced by name lookups.
//!
//! `T1.5` makes the real pack additive only: a property change is a new record
//! version, never an edit in place, and removal is deprecation.

mod hash;
mod pack;
mod record;

pub use hash::{KIND_BLOCK, record_hash, render_hash};
pub use pack::{ContentPack, UNKNOWN_SKY_ATTEN};
pub use record::{
    BlockRecord, FaceTextures, LAMP_EMISSION, LIGHT_LEVELS, MAX_NAME_BYTES, MAX_RECORDS,
    RenderLayer, RenderShape, UNKNOWN_BASE, WATER_SKY_ATTEN, v1_default,
};
