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

use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{ErrorKind, SectionFault, Stage, TerrainError};

/// File name of the world header inside a world directory.
pub const HEADER_FILE: &str = "header.bin";

/// First four bytes of every header.
pub const MAGIC: [u8; 4] = *b"TWLD";

/// Header/save format version this build writes (`A1.2`).
///
/// Version 1 has no migration state; version 2 appends it. Both are read.
pub const HEADER_FORMAT_VERSION: u16 = 2;

/// The oldest header layout this build still reads.
pub const HEADER_FORMAT_V1: u16 = 1;

/// Hash algorithm recorded beside `description_hash` (`A1.2`, `A3.5`).
pub const HASH_ALGORITHM: &str = "sha256/v1";

/// Longest string one length-prefixed header field can carry.
const MAX_TEXT_BYTES: usize = u8::MAX as usize;

/// Number of steps in the load order (`A1.5`).
pub const LOAD_STEP_COUNT: usize = 7;

/// Migration state carried by the header (`A16`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MigrationState {
    /// No migration is in progress.
    None,
    /// A migration opened the world in resume mode.
    InProgress {
        /// Format the world is migrating from.
        from_format: u16,
        /// Format the world is migrating to.
        to_format: u16,
        /// UTC Unix seconds when the migration began.
        started: i64,
    },
}

impl MigrationState {
    /// The on-disk tag.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::None => 0,
            Self::InProgress { .. } => 1,
        }
    }

    /// Whether a migration must be resumed before the world opens.
    #[must_use]
    pub const fn is_in_progress(self) -> bool {
        matches!(self, Self::InProgress { .. })
    }
}

/// The mandatory world header (`A1.2`).
///
/// Seed and platform tag are here rather than in the description because one
/// description serves many worlds (`A1.1`). `description_hash` is the 32-byte
/// SHA-256 digest that `Description::description_hash` returns, so the header and
/// `ErrorKind::DescriptionHashMismatch` carry the same width.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Header {
    /// On-disk format version. A reader refuses any value it does not know.
    pub format_version: u16,
    /// Generator version this world is pinned to (`T1.2`).
    pub generator_version: u16,
    /// Digest of the canonical description (`A1.1`, `A3.5`).
    pub description_hash: [u8; 32],
    /// Algorithm `description_hash` was produced with (`A3.5`).
    pub hash_algorithm: String,
    /// World seed. Not part of the description hash (`T1.2`).
    pub seed: u64,
    /// Target triple of first generation (`A2.6`), display and diagnostics only.
    pub platform_tag: String,
    /// Target triple of the most recent save (`A2.6`), display only.
    pub last_saved_platform_tag: String,
    /// 128-bit world identity, never reused; a Fork assigns a new one (`A1.2`).
    pub world_id: [u8; 16],
    /// Display name. Never identity, never a path.
    pub name: String,
    /// Creation time, UTC Unix seconds (`A2.4` allows clocks in timestamps only).
    pub created: i64,
    /// Most recent save time, UTC Unix seconds.
    pub last_saved: i64,
    /// Monotonic committed-edit revision (`J1.2`).
    pub revision: u64,
    /// Description schema version this world was written against.
    pub schema_version: u16,
    /// Migration state; version 1 headers decode as [`MigrationState::None`].
    pub migration_state: MigrationState,
}

impl Header {
    /// Encodes the header, checksum last.
    ///
    /// Total and allocation-only: a string longer than 255 bytes is truncated
    /// at a character boundary so this cannot panic. [`Self::validate`] is the
    /// refusal point and runs before [`Self::write`] touches the disk.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = self.encode_body();
        let checksum = crc32(&out);
        out.extend_from_slice(&checksum.to_le_bytes());
        out
    }

    /// Encodes the version 1 layout, for migration fixtures and tests.
    ///
    /// Version 1 has no `migration_state` field. Production code writes the
    /// current layout through [`Self::write`].
    #[must_use]
    pub fn encode_v1(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(128);
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&HEADER_FORMAT_V1.to_le_bytes());
        out.extend_from_slice(&self.generator_version.to_le_bytes());
        out.extend_from_slice(&self.description_hash);
        push_text(&mut out, &self.hash_algorithm);
        out.extend_from_slice(&self.seed.to_le_bytes());
        push_text(&mut out, &self.platform_tag);
        push_text(&mut out, &self.last_saved_platform_tag);
        out.extend_from_slice(&self.world_id);
        push_text(&mut out, &self.name);
        out.extend_from_slice(&self.created.to_le_bytes());
        out.extend_from_slice(&self.last_saved.to_le_bytes());
        out.extend_from_slice(&self.revision.to_le_bytes());
        out.extend_from_slice(&self.schema_version.to_le_bytes());
        let checksum = crc32(&out);
        out.extend_from_slice(&checksum.to_le_bytes());
        out
    }

    /// The version-2 body before its checksum.
    fn encode_body(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(128);
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&self.format_version.to_le_bytes());
        out.extend_from_slice(&self.generator_version.to_le_bytes());
        out.extend_from_slice(&self.description_hash);
        push_text(&mut out, &self.hash_algorithm);
        out.extend_from_slice(&self.seed.to_le_bytes());
        push_text(&mut out, &self.platform_tag);
        push_text(&mut out, &self.last_saved_platform_tag);
        out.extend_from_slice(&self.world_id);
        push_text(&mut out, &self.name);
        out.extend_from_slice(&self.created.to_le_bytes());
        out.extend_from_slice(&self.last_saved.to_le_bytes());
        out.extend_from_slice(&self.revision.to_le_bytes());
        out.extend_from_slice(&self.schema_version.to_le_bytes());
        match self.migration_state {
            MigrationState::None => out.push(0),
            MigrationState::InProgress {
                from_format,
                to_format,
                started,
            } => {
                out.push(1);
                out.extend_from_slice(&from_format.to_le_bytes());
                out.extend_from_slice(&to_format.to_le_bytes());
                out.extend_from_slice(&started.to_le_bytes());
            }
        }
        out
    }

    /// Decodes header bytes, refusing anything this build cannot interpret.
    ///
    /// `format_version` is read before the rest of the body: a future format
    /// may move or lengthen the fields, so its checksum span is unknown here
    /// and the only honest answer is `NewerThanEngine`. For every format this
    /// build knows, the body is framed, the trailing-bytes check runs, then the
    /// checksum (`A1.5`'s first step), and no field is trusted before it
    /// matches. Errors use `CorruptSection` with the header's file name because
    /// there is no header-specific error kind.
    ///
    /// # Errors
    ///
    /// `SectionFault::Length` for a short buffer or trailing bytes,
    /// `SectionFault::Framing` for a wrong magic or an unreadable field,
    /// `SectionFault::Checksum` for a bad `crc32`, and
    /// `ErrorKind::NewerThanEngine` for a newer `format_version`.
    pub fn decode(bytes: &[u8]) -> Result<Self, TerrainError> {
        decode_at(bytes, Path::new(HEADER_FILE))
    }

    /// Reads and decodes `path`.
    ///
    /// # Errors
    ///
    /// Returns an `IoError` when the file cannot be read, otherwise whatever
    /// [`Self::decode`] reports.
    pub fn read(path: &Path) -> Result<Self, TerrainError> {
        let bytes = std::fs::read(path).map_err(|error| io_error("read", path, &error))?;
        decode_at(&bytes, path)
    }

    /// Writes `path` through a temp file, `fsync`, rename, and directory
    /// `fsync` (`J3.4`). On Windows the directory `fsync` is a no-op.
    ///
    /// # Errors
    ///
    /// Refuses an unencodable field, or returns an `IoError` from any step.
    /// A failure before the rename removes the temp file and leaves the
    /// destination alone; a directory-`fsync` failure reports an error even
    /// though the rename already landed.
    pub fn write(&self, path: &Path) -> Result<(), TerrainError> {
        self.validate()?;
        write_atomic(path, &self.encode())
    }

    /// Checks that every field can be framed by the one-byte length prefixes.
    ///
    /// The header has no dedicated field-rule error kind, so a caller-supplied
    /// value that cannot be encoded is reported as a `DescriptionInvalid` on
    /// the offending key.
    ///
    /// # Errors
    ///
    /// `DescriptionInvalid` for a string longer than 255 bytes or an empty
    /// `hash_algorithm`.
    pub fn validate(&self) -> Result<(), TerrainError> {
        for (key, text) in [
            ("hash_algorithm", &self.hash_algorithm),
            ("platform_tag", &self.platform_tag),
            ("last_saved_platform_tag", &self.last_saved_platform_tag),
            ("name", &self.name),
        ] {
            if text.len() > MAX_TEXT_BYTES {
                return Err(field_invalid(key, "max 255 bytes"));
            }
        }
        if self.hash_algorithm.is_empty() {
            return Err(field_invalid("hash_algorithm", "non-empty"));
        }
        if self.format_version != HEADER_FORMAT_VERSION {
            return Err(field_invalid("format_version", "current"));
        }
        match self.migration_state {
            MigrationState::InProgress {
                from_format,
                to_format,
                ..
            } if from_format >= to_format => {
                return Err(field_invalid("migration_state", "from < to"));
            }
            _ => {}
        }
        Ok(())
    }
}

/// The current UTC time in Unix seconds, for `created` and `last_saved`.
///
/// `A2.4` allows a clock in header timestamps and nowhere that reaches a block,
/// a field, a hash, or a cache key.
pub fn now_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

/// One step of the world load order (`A1.5`).
///
/// Closed: adding a step is a change to the answer, not a data edit. There is
/// deliberately no "create" step.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum LoadStep {
    /// The header reads and its `crc32` matches.
    HeaderChecksum,
    /// The header `format_version` is supported by this build.
    FormatSupport,
    /// `description.toml` is present and parses.
    DescriptionPresent,
    /// The recomputed description hash matches the stored one.
    DescriptionHash,
    /// `generator_version` is current or still evaluable.
    GeneratorVersion,
    /// The pinned graph and content manifest are valid for this build.
    Manifest,
    /// A torn journal tail is truncated and covered records are replayed.
    JournalRecovery,
}

/// The exact refusal order (`A1.5`).
///
/// Header crc, format support, description present and parsing, recomputed
/// hash, generator version support, manifest validation, journal recovery.
pub const fn load_order() -> [LoadStep; LOAD_STEP_COUNT] {
    [
        LoadStep::HeaderChecksum,
        LoadStep::FormatSupport,
        LoadStep::DescriptionPresent,
        LoadStep::DescriptionHash,
        LoadStep::GeneratorVersion,
        LoadStep::Manifest,
        LoadStep::JournalRecovery,
    ]
}

/// The load step that refused `err`, or `None` when the error is not a load
/// refusal.
///
/// `A1.5`: the first failure ends the load, and the directory stays
/// byte-identical. A header checksum or header I/O failure refuses at
/// [`LoadStep::HeaderChecksum`]; a section checksum later in the session is not
/// a load refusal and maps to `None`.
pub fn refuse_reason(err: &TerrainError) -> Option<LoadStep> {
    match &err.kind {
        ErrorKind::CorruptSection { path, .. } | ErrorKind::IoError { path, .. }
            if is_header_file(path) =>
        {
            Some(LoadStep::HeaderChecksum)
        }
        ErrorKind::NewerThanEngine { .. } if err.stage == Stage::Persistence => {
            Some(LoadStep::FormatSupport)
        }
        ErrorKind::NewerThanEngine { .. } | ErrorKind::UnknownGeneratorVersion(_) => {
            Some(LoadStep::GeneratorVersion)
        }
        ErrorKind::DescriptionHashMismatch { .. } => Some(LoadStep::DescriptionHash),
        ErrorKind::DescriptionInvalid { .. } | ErrorKind::IoError { .. }
            if err.stage == Stage::Description =>
        {
            Some(LoadStep::DescriptionPresent)
        }
        ErrorKind::GraphInvalid { .. } => Some(LoadStep::Manifest),
        ErrorKind::JournalTornTail { .. } => Some(LoadStep::JournalRecovery),
        _ => None,
    }
}

/// `crc32` with the reflected IEEE polynomial `0xEDB8_8320`.
///
/// Hand-written so the header needs no dependency. The published check value
/// for `"123456789"` is `0xCBF4_3926`.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for &byte in bytes {
        let index = ((crc ^ u32::from(byte)) & 0xFF) as usize;
        crc = (crc >> 8) ^ CRC32_TABLE[index];
    }
    !crc
}

/// The reflected CRC-32 lookup table, built at compile time.
const CRC32_TABLE: [u32; 256] = build_crc32_table();

const fn build_crc32_table() -> [u32; 256] {
    let mut table = [0_u32; 256];
    let mut index = 0_u32;
    while index < 256 {
        let mut crc = index;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[index as usize] = crc;
        index += 1;
    }
    table
}

fn is_header_file(path: &Path) -> bool {
    path.file_name()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|name| name == HEADER_FILE)
}

fn corrupt(path: &Path, fault: SectionFault) -> TerrainError {
    TerrainError::new(
        Stage::Persistence,
        ErrorKind::CorruptSection {
            path: path.to_path_buf(),
            fault,
        },
    )
}

fn field_invalid(key: &'static str, rule: &'static str) -> TerrainError {
    TerrainError::new(
        Stage::Persistence,
        ErrorKind::DescriptionInvalid { key, rule },
    )
}

fn io_error(op: &'static str, path: &Path, error: &std::io::Error) -> TerrainError {
    TerrainError::new(
        Stage::Persistence,
        ErrorKind::IoError {
            op,
            path: path.to_path_buf(),
            os_error: error.raw_os_error().unwrap_or(0),
        },
    )
}

/// Appends one length-prefixed text field.
fn push_text(out: &mut Vec<u8>, text: &str) {
    let bytes = text_bytes(text);
    out.push(u8::try_from(bytes.len()).unwrap_or(u8::MAX));
    out.extend_from_slice(bytes);
}

/// The bytes a text field can carry, cut at a character boundary at 255.
fn text_bytes(text: &str) -> &[u8] {
    let bytes = text.as_bytes();
    let mut end = bytes.len().min(MAX_TEXT_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &bytes[..end]
}

fn decode_at(bytes: &[u8], path: &Path) -> Result<Header, TerrainError> {
    let fail = |fault| corrupt(path, fault);
    let Some(magic) = bytes.get(..MAGIC.len()) else {
        return Err(fail(SectionFault::Length));
    };
    if magic != MAGIC {
        return Err(fail(SectionFault::Framing));
    }

    let mut cursor = Cursor {
        bytes,
        pos: MAGIC.len(),
    };
    let format_version = cursor.u16().map_err(fail)?;
    if format_version > HEADER_FORMAT_VERSION {
        // The checksum span of a future layout is unknown, so this build
        // cannot verify the bytes it is refusing. That is still the truthful
        // answer: the world needs a newer engine.
        return Err(TerrainError::new(
            Stage::Persistence,
            ErrorKind::NewerThanEngine {
                found: format_version,
                engine: HEADER_FORMAT_VERSION,
            },
        ));
    }
    let header = Header {
        format_version,
        generator_version: cursor.u16().map_err(fail)?,
        description_hash: cursor.array().map_err(fail)?,
        hash_algorithm: cursor.text().map_err(fail)?,
        seed: cursor.u64().map_err(fail)?,
        platform_tag: cursor.text().map_err(fail)?,
        last_saved_platform_tag: cursor.text().map_err(fail)?,
        world_id: cursor.array().map_err(fail)?,
        name: cursor.text().map_err(fail)?,
        created: cursor.i64().map_err(fail)?,
        last_saved: cursor.i64().map_err(fail)?,
        revision: cursor.u64().map_err(fail)?,
        schema_version: cursor.u16().map_err(fail)?,
        migration_state: if format_version >= 2 {
            match cursor.u8().map_err(fail)? {
                0 => MigrationState::None,
                1 => MigrationState::InProgress {
                    from_format: cursor.u16().map_err(fail)?,
                    to_format: cursor.u16().map_err(fail)?,
                    started: cursor.i64().map_err(fail)?,
                },
                _ => return Err(fail(SectionFault::Framing)),
            }
        } else {
            MigrationState::None
        },
    };

    let body_end = cursor.pos;
    if bytes.len() != body_end.saturating_add(4) {
        return Err(fail(SectionFault::Length));
    }
    let stored = cursor.u32().map_err(fail)?;
    if crc32(&bytes[..body_end]) != stored {
        return Err(fail(SectionFault::Checksum));
    }

    if header.format_version < HEADER_FORMAT_V1
        || header.format_version > HEADER_FORMAT_VERSION
        || header.hash_algorithm.is_empty()
    {
        return Err(fail(SectionFault::Framing));
    }
    Ok(header)
}

/// A bounds-checked reader over header bytes.
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], SectionFault> {
        let end = self.pos.checked_add(count).ok_or(SectionFault::Length)?;
        let slice = self.bytes.get(self.pos..end).ok_or(SectionFault::Length)?;
        self.pos = end;
        Ok(slice)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], SectionFault> {
        self.take(N)?.try_into().map_err(|_| SectionFault::Framing)
    }

    fn u16(&mut self) -> Result<u16, SectionFault> {
        self.array().map(u16::from_le_bytes)
    }

    fn u32(&mut self) -> Result<u32, SectionFault> {
        self.array().map(u32::from_le_bytes)
    }

    fn u64(&mut self) -> Result<u64, SectionFault> {
        self.array().map(u64::from_le_bytes)
    }

    fn i64(&mut self) -> Result<i64, SectionFault> {
        self.array().map(i64::from_le_bytes)
    }

    fn u8(&mut self) -> Result<u8, SectionFault> {
        let byte = *self.take(1)?.first().ok_or(SectionFault::Length)?;
        Ok(byte)
    }

    fn text(&mut self) -> Result<String, SectionFault> {
        let length = usize::from(self.u8()?);
        let raw = self.take(length)?;
        std::str::from_utf8(raw)
            .map(str::to_owned)
            .map_err(|_| SectionFault::Framing)
    }
}

/// Writes through a temp file, `fsync`, rename, then directory `fsync`.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), TerrainError> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let temp = temp_path(path);
    let outcome = stage_and_swap(&temp, path, bytes).and_then(|()| {
        sync_directory(directory).map_err(|error| io_error("fsync", directory, &error))
    });
    if outcome.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    outcome
}

fn stage_and_swap(temp: &Path, path: &Path, bytes: &[u8]) -> Result<(), TerrainError> {
    let mut file = std::fs::File::create(temp).map_err(|error| io_error("create", temp, &error))?;
    file.write_all(bytes)
        .map_err(|error| io_error("write", temp, &error))?;
    file.sync_all()
        .map_err(|error| io_error("fsync", temp, &error))?;
    drop(file);
    std::fs::rename(temp, path).map_err(|error| io_error("rename", path, &error))
}

/// `fsync`s a directory so the rename itself is durable.
#[cfg(unix)]
fn sync_directory(directory: &Path) -> std::io::Result<()> {
    std::fs::File::open(directory)?.sync_all()
}

/// Directory `fsync` is a no-op on Windows: `std` cannot open a directory
/// handle, so the rename is the durability point there (`J3.4`).
///
/// The result-shaped signature mirrors the Unix path so [`write_atomic`] has a
/// single code path on every target.
#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn sync_directory(_directory: &Path) -> std::io::Result<()> {
    Ok(())
}

fn temp_path(path: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    let mut name = path
        .file_name()
        .map_or_else(|| OsString::from(HEADER_FILE), OsString::from);
    name.push(format!(".{}.{sequence}.tmp", std::process::id()));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn sample() -> Header {
        Header {
            format_version: HEADER_FORMAT_VERSION,
            generator_version: 1,
            description_hash: [0x11; 32],
            hash_algorithm: HASH_ALGORITHM.to_owned(),
            seed: 0x0123_4567_89AB_CDEF,
            platform_tag: "x86_64-pc-windows-msvc".to_owned(),
            last_saved_platform_tag: "aarch64-apple-darwin".to_owned(),
            world_id: [0x5A; 16],
            name: "Test World".to_owned(),
            created: 1_700_000_000,
            last_saved: 1_700_003_600,
            revision: 42,
            schema_version: 1,
            migration_state: MigrationState::None,
        }
    }

    fn temp_directory(name: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "valescent-header-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn fault(error: &TerrainError) -> SectionFault {
        match &error.kind {
            ErrorKind::CorruptSection { fault, .. } => *fault,
            other => panic!("expected CorruptSection, found {other:?}"),
        }
    }

    #[test]
    fn header_round_trips() {
        let header = sample();
        let encoded = header.encode();
        let decoded = Header::decode(&encoded).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(decoded, header);
        assert_eq!(decoded.encode(), encoded, "encode is canonical");
    }

    #[test]
    fn version_1_headers_decode_with_no_migration_state() {
        let header = sample();
        let decoded = Header::decode(&header.encode_v1()).expect("a v1 header reads");
        assert_eq!(decoded.format_version, HEADER_FORMAT_V1);
        assert_eq!(decoded.migration_state, MigrationState::None);
        assert_eq!(decoded.seed, header.seed);
        assert_eq!(decoded.name, header.name);
    }

    #[test]
    fn migration_state_round_trips() {
        let mut header = sample();
        header.migration_state = MigrationState::InProgress {
            from_format: 1,
            to_format: 2,
            started: 1_700_000_000,
        };
        let decoded = Header::decode(&header.encode()).expect("a v2 header reads");
        assert_eq!(decoded.migration_state, header.migration_state);
    }

    #[test]
    fn validate_refuses_old_or_impossible_headers() {
        let mut old = sample();
        old.format_version = HEADER_FORMAT_V1;
        assert!(old.validate().is_err(), "writes use the current format");

        let mut backwards = sample();
        backwards.migration_state = MigrationState::InProgress {
            from_format: 2,
            to_format: 1,
            started: 0,
        };
        assert!(backwards.validate().is_err(), "migration must move forward");
    }

    #[test]
    fn magic_and_crc_are_checked() {
        let encoded = sample().encode();

        let mut wrong_magic = encoded.clone();
        wrong_magic[0] = b'X';
        assert_eq!(
            fault(&Header::decode(&wrong_magic).unwrap_err()),
            SectionFault::Framing
        );

        // `schema_version` sits six bytes from the end (after revision, before
        // the crc) and has no semantic check, so a flipped byte must surface as
        // a checksum failure.
        let mut flipped = encoded.clone();
        let schema_byte = flipped.len() - 6;
        flipped[schema_byte] ^= 0xFF;
        let error = Header::decode(&flipped).unwrap_err();
        assert_eq!(fault(&error), SectionFault::Checksum);
        assert_eq!(
            error.stage,
            Stage::Persistence,
            "header faults are persistence faults"
        );

        assert_eq!(
            fault(&Header::decode(&encoded[..10]).unwrap_err()),
            SectionFault::Length,
            "a short buffer is a length fault"
        );
        assert_eq!(
            fault(&Header::decode(&[0xFF; 3]).unwrap_err()),
            SectionFault::Length,
            "three junk bytes must not panic"
        );
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let encoded = sample().encode();

        let mut one_extra = encoded.clone();
        one_extra.push(0);
        assert_eq!(
            fault(&Header::decode(&one_extra).unwrap_err()),
            SectionFault::Length
        );

        let mut four_extra = encoded.clone();
        four_extra.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(
            fault(&Header::decode(&four_extra).unwrap_err()),
            SectionFault::Length
        );
    }

    #[test]
    fn newer_format_refuses_with_newer_than_engine() {
        let header = Header {
            format_version: HEADER_FORMAT_VERSION + 1,
            ..sample()
        };
        let error = Header::decode(&header.encode()).unwrap_err();
        assert_eq!(
            error.kind,
            ErrorKind::NewerThanEngine {
                found: HEADER_FORMAT_VERSION + 1,
                engine: HEADER_FORMAT_VERSION,
            }
        );
        assert_eq!(
            refuse_reason(&error),
            Some(LoadStep::FormatSupport),
            "a newer header format is refused at format support"
        );

        // A future layout may be longer, so its checksum span is unknown. The
        // version is read before the body and must still name the right step
        // rather than reporting a corrupt v1 header.
        let mut future_layout = header.encode();
        future_layout.extend_from_slice(&[0xAB; 4]);
        let error = Header::decode(&future_layout).unwrap_err();
        assert!(matches!(error.kind, ErrorKind::NewerThanEngine { .. }));

        // Version 0 predates this build and is not a future layout.
        let mut older = sample().encode_v1();
        older[4..6].copy_from_slice(&0_u16.to_le_bytes());
        let checksum = crc32(&older[..older.len() - 4]);
        let length = older.len();
        older[length - 4..].copy_from_slice(&checksum.to_le_bytes());
        assert_eq!(
            fault(&Header::decode(&older).unwrap_err()),
            SectionFault::Framing
        );
    }

    #[test]
    fn header_corruption_never_panics() {
        let bytes = sample().encode();
        for index in 0..bytes.len() {
            let mut damaged = bytes.clone();
            damaged[index] ^= 0xFF;
            let _ = Header::decode(&damaged);
        }
    }

    #[test]
    fn crc32_matches_the_published_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"a"), 0xE8B7_BE43);
        assert_eq!(crc32(b"abc"), 0x3524_41C2);
    }

    #[test]
    fn load_order_matches_the_answer() {
        assert_eq!(
            load_order(),
            [
                LoadStep::HeaderChecksum,
                LoadStep::FormatSupport,
                LoadStep::DescriptionPresent,
                LoadStep::DescriptionHash,
                LoadStep::GeneratorVersion,
                LoadStep::Manifest,
                LoadStep::JournalRecovery,
            ]
        );

        // Exhaustive, so adding or removing a step breaks this test: the order
        // is a closed answer, and no arm creates a file.
        for step in load_order() {
            let label = match step {
                LoadStep::HeaderChecksum => "header crc",
                LoadStep::FormatSupport => "format support",
                LoadStep::DescriptionPresent => "description present",
                LoadStep::DescriptionHash => "description hash",
                LoadStep::GeneratorVersion => "generator version",
                LoadStep::Manifest => "manifest",
                LoadStep::JournalRecovery => "journal recovery",
            };
            assert!(!label.is_empty(), "{step:?}");
        }
    }

    #[test]
    fn refusal_maps_to_the_first_failing_step() {
        let corrupt_header = Header::decode(&{
            let mut bytes = sample().encode();
            let schema_byte = bytes.len() - 6;
            bytes[schema_byte] ^= 0xFF;
            bytes
        })
        .unwrap_err();
        assert_eq!(
            refuse_reason(&corrupt_header),
            Some(LoadStep::HeaderChecksum)
        );

        let missing_header = Header::read(Path::new("no-such-world/header.bin")).unwrap_err();
        assert_eq!(
            refuse_reason(&missing_header),
            Some(LoadStep::HeaderChecksum)
        );

        let newer_generator = TerrainError::new(
            Stage::Migration,
            ErrorKind::NewerThanEngine {
                found: 9,
                engine: 2,
            },
        );
        assert_eq!(
            refuse_reason(&newer_generator),
            Some(LoadStep::GeneratorVersion)
        );
        assert_eq!(
            refuse_reason(&TerrainError::new(
                Stage::Description,
                ErrorKind::UnknownGeneratorVersion(9)
            )),
            Some(LoadStep::GeneratorVersion)
        );

        assert_eq!(
            refuse_reason(&TerrainError::new(
                Stage::Description,
                ErrorKind::DescriptionInvalid {
                    key: "SEA",
                    rule: "range",
                },
            )),
            Some(LoadStep::DescriptionPresent)
        );

        assert_eq!(
            refuse_reason(&TerrainError::new(
                Stage::Description,
                ErrorKind::DescriptionHashMismatch {
                    expected: [1; 32],
                    found: [2; 32],
                },
            )),
            Some(LoadStep::DescriptionHash)
        );

        assert_eq!(
            refuse_reason(&TerrainError::new(
                Stage::Graph,
                ErrorKind::GraphInvalid {
                    node: 3,
                    rule: "cycle",
                },
            )),
            Some(LoadStep::Manifest)
        );

        assert_eq!(
            refuse_reason(&TerrainError::new(
                Stage::Persistence,
                ErrorKind::JournalTornTail {
                    path: PathBuf::from("world/journal.log"),
                    valid_bytes: 12,
                },
            )),
            Some(LoadStep::JournalRecovery)
        );

        assert_eq!(
            refuse_reason(&TerrainError::new(
                Stage::Description,
                ErrorKind::IoError {
                    op: "read",
                    path: PathBuf::from("world/description.toml"),
                    os_error: 2,
                },
            )),
            Some(LoadStep::DescriptionPresent)
        );
    }

    #[test]
    fn non_load_failures_have_no_step() {
        assert_eq!(
            refuse_reason(&TerrainError::new(
                Stage::Persistence,
                ErrorKind::CorruptSection {
                    path: PathBuf::from("world/chunks/c.0.0.0.sect"),
                    fault: SectionFault::Checksum,
                },
            )),
            None,
            "a saved-section failure is not a load refusal"
        );
        assert_eq!(
            refuse_reason(&TerrainError::new(
                Stage::Height,
                ErrorKind::JobFailed {
                    stage: Stage::Height,
                },
            )),
            None
        );
        assert_eq!(
            refuse_reason(&TerrainError::new(
                Stage::Persistence,
                ErrorKind::DescriptionInvalid {
                    key: "name",
                    rule: "max 255 bytes",
                },
            )),
            None,
            "a header write refusal is not a load refusal"
        );
    }

    #[test]
    fn write_is_atomic_and_leaves_no_temp_file() {
        let directory = temp_directory("atomic");
        let path = directory.join(HEADER_FILE);
        let header = sample();

        header.write(&path).unwrap();
        assert!(path.exists(), "the header must be at its destination");
        assert_eq!(Header::read(&path).unwrap(), header);

        let leftovers: Vec<PathBuf> = fs::read_dir(&directory)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|entry| entry.to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "a completed write leaves no temp file, found {leftovers:?}"
        );

        // Overwriting goes through the same rename, so still one file and no temp.
        let mut second = header.clone();
        second.revision = 43;
        second.write(&path).unwrap();
        assert_eq!(Header::read(&path).unwrap().revision, 43);
        assert_eq!(
            fs::read_dir(&directory).unwrap().count(),
            1,
            "only header.bin remains"
        );

        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn refusal_never_creates_anything() {
        let directory = temp_directory("no-create");
        let path = directory.join(HEADER_FILE);

        let error = Header::read(&path).unwrap_err();
        assert_eq!(refuse_reason(&error), Some(LoadStep::HeaderChecksum));
        assert!(!path.exists(), "the loader must not create a header");
        assert_eq!(
            fs::read_dir(&directory).unwrap().count(),
            0,
            "a refused load leaves the directory byte-identical"
        );

        let mut header = sample();
        header.name = "x".repeat(MAX_TEXT_BYTES + 1);
        assert!(
            header.validate().is_err(),
            "an unframeable field is refused"
        );
        assert!(header.write(&path).is_err());
        assert!(!path.exists(), "a refused write creates nothing");

        let _ = fs::remove_dir_all(directory);
    }
}
