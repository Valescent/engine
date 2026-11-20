//! Edit records, total order, journal framing, and the group-commit writer
//! (`J1`, `J2`, `J3`).
//!
//! The edit log is the overlay over generated terrain: an append-only list of
//! per-block changes, each carrying the value it replaced. This module owns the
//! records, their total order, the bytes they travel in, and the policy that
//! decides when those bytes reach a sink. It never opens a file and never
//! spawns a thread (`J3.1`): a caller supplies a [`JournalSink`] (a
//! [`std::fs::File`] for the real journal, a `Vec<u8>` in tests), builds an
//! [`EditBatch`], pushes it into a [`JournalWriter`], and recovers with
//! [`decode_batches`].
//!
//! Three rules drive every shape here:
//!
//! - Names are persisted, ids are not (`J1.1`, `A5.1`, `T1.6`). A record stores
//!   a [`BlockName`], so a world can be read by a build whose content pack does
//!   not know the name and the name survives the round trip byte-for-byte.
//! - The order is `(revision, batch, seq)` (`J2.1`). Position is not part of
//!   it, so a sort never decides between two records at different coordinates.
//! - Nothing is recorded when `old == new` (`J1.4`). The [`Edit::record`]
//!   constructor, [`EditBatch::new`], [`EditBatch::commit`], and
//!   [`encode_batch`] each drop such a pair.
//!
//! # Wire layout (`J3.3`)
//!
//! ```text
//! frame:
//!   len      u32 LE   payload byte count; never includes `revision` or `crc`
//!   revision u64 LE   revision shared by every record in the frame
//!   payload  [u8]     see below
//!   crc      u32 LE   IEEE crc32 over the `revision` bytes, then the payload
//!
//! payload:
//!   author   u32 LE   EditBatch::author
//!   count    u32 LE   number of records
//!   record, `count` times:
//!     x, y, z  i32 LE each
//!     seq      u32 LE
//!     author   u32 LE
//!     batch    u32 LE
//!     old_len  u32 LE, then `old_len` bytes of UTF-8
//!     new_len  u32 LE, then `new_len` bytes of UTF-8
//! ```
//!
//! An edit's `revision` is the frame revision and is not stored per record,
//! because `J1.2` assigns one revision per committed batch. The crc is IEEE
//! (`0xEDB8_8320`, reflected); its check value over `b"123456789"` is
//! `0xCBF4_3926`. A frame is complete or it is not: a length that runs past the
//! end of the buffer is a torn tail, never an error.
//!
//! # Group commit (`J3.1`)
//!
//! [`encode_batch`] and [`decode_batches`] are pure framing; [`JournalWriter`]
//! adds the policy. It buffers encoded frames and flushes them in one
//! [`JournalSink::write_all`] when [`CommitPolicy::should_flush`] says so (64
//! batches or 250 ms, whichever comes first), and calls
//! [`JournalSink::sync_data`] only when [`CommitPolicy::should_sync`] says so
//! (every 10 s) or on an explicit [`JournalWriter::flush`] (pause, quit, before
//! a section save).
//!
//! No clock is read in this module. Every time value is a caller-supplied
//! monotonic millisecond count sharing an origin with the writer's creation;
//! the writer starts that clock at zero. The IO thread owns the sink and the
//! file: it recovers the torn tail, appends from that offset, calls
//! [`JournalWriter::push`] on the producing side, polls
//! [`JournalWriter::flush_if_due`] with its current count, and calls
//! [`JournalWriter::flush`] at the explicit barriers.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use crate::error::{Coord, ErrorKind, Stage, TerrainError};

/// A stable block name, never a runtime id (`J1.1`, `A5.1`).
pub type BlockName = String;

/// `len` (4) + `revision` (8) + `crc` (4).
const FRAME_OVERHEAD: usize = 16;

/// `len` (4) + `revision` (8).
const HEADER_BYTES: usize = 12;

/// Smallest possible record: three `i32`, three `u32`, two empty names.
const MIN_RECORD_BYTES: usize = 32;

/// One appended block change.
///
/// Records are append-only. The same coordinate may appear many times; the
/// read path is [`last_write_wins`] and [`coalesce`]. Coordinates are world
/// block coordinates, so a chunk-key layout change never rewrites the log
/// (`A4.2`).
#[derive(Clone, Debug)]
pub struct Edit {
    /// World block `x`.
    pub x: i32,
    /// World block `y`.
    pub y: i32,
    /// World block `z`.
    pub z: i32,
    /// Stable name of the value that was replaced.
    pub old: BlockName,
    /// Stable name of the value that was written.
    pub new: BlockName,
    /// Committed revision of the batch that wrote this record (`J1.2`).
    pub revision: u64,
    /// Stable player id; `0` is the system or a tool (`J1.3`).
    pub author: u32,
    /// Sub-batch index within the revision (`J2.1`).
    pub batch: u32,
    /// Position of this record within its batch (`J2.1`).
    pub seq: u32,
}

impl Edit {
    /// Builds one record, or `None` when the change is a no-op (`J1.4`).
    ///
    /// Every committed change is recorded, including air to block and block to
    /// air; only `old == new` is dropped. The struct fields are public so a
    /// decoder can rebuild a record without renaming it, but this constructor
    /// is the only place that decides whether a record exists.
    #[must_use]
    pub fn record(
        position: Coord,
        old: impl Into<BlockName>,
        new: impl Into<BlockName>,
        revision: u64,
        author: u32,
        batch: u32,
        seq: u32,
    ) -> Option<Self> {
        let old = old.into();
        let new = new.into();
        if old == new {
            return None;
        }
        Some(Self {
            x: position[0],
            y: position[1],
            z: position[2],
            old,
            new,
            revision,
            author,
            batch,
            seq,
        })
    }

    /// World block coordinate.
    #[must_use]
    pub const fn position(&self) -> Coord {
        [self.x, self.y, self.z]
    }

    /// The total-order key `(revision, batch, seq)` (`J2.1`).
    #[must_use]
    pub const fn order_key(&self) -> (u64, u32, u32) {
        (self.revision, self.batch, self.seq)
    }

    /// Compares every field, including the position and both names.
    ///
    /// [`PartialEq`] deliberately compares the order key only, so this is the
    /// way to ask "is this the same record?" rather than "is this the same
    /// point in the log?".
    #[must_use]
    pub fn same_record(&self, other: &Self) -> bool {
        self.x == other.x
            && self.y == other.y
            && self.z == other.z
            && self.old == other.old
            && self.new == other.new
            && self.revision == other.revision
            && self.author == other.author
            && self.batch == other.batch
            && self.seq == other.seq
    }
}

/// Total order `(revision, batch, seq)` (`J2.1`).
///
/// Position and names are **not** part of the order: two records that differ
/// only in `x`/`y`/`z` compare [`Ordering::Equal`], which is why callers that
/// care about coordinates key by [`Edit::position`] (as [`last_write_wins`]
/// and [`coalesce`] do). Equal keys in one log are an invariant violation
/// (`J2.1`); a stable sort therefore only ever sees them from bad input, and
/// keeps them in arrival order.
impl Ord for Edit {
    fn cmp(&self, other: &Self) -> Ordering {
        self.order_key().cmp(&other.order_key())
    }
}

impl PartialOrd for Edit {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Equality on the total-order key, so [`Eq`] and [`Ord`] agree.
///
/// This is *not* record equality; use [`Edit::same_record`] for that.
impl PartialEq for Edit {
    fn eq(&self, other: &Self) -> bool {
        self.order_key() == other.order_key()
    }
}

impl Eq for Edit {}

/// One committed revision of edits (`J1.1`–`J1.4`).
///
/// A batch is authored once (`author`) and revisioned once (`revision`); every
/// record inside it shares both. [`commit`](Self::commit) is what turns a
/// pending batch into a committed one.
#[derive(Clone, Debug)]
pub struct EditBatch {
    /// Revision of this committed batch (`J1.2`).
    pub revision: u64,
    /// Stable player id for the whole batch; `0` is the system (`J1.3`).
    pub author: u32,
    /// Records in the order they were applied; `seq` is their position.
    pub edits: Vec<Edit>,
}

impl EditBatch {
    /// Builds a committed batch, dropping no-ops and renumbering `seq`.
    ///
    /// This is the enforcing constructor for `J1.4`: a record whose `old` and
    /// `new` are the same name never enters the batch. `seq` is renumbered to
    /// the record's position because `J2.1` defines it as exactly that; a
    /// batch built by hand with other keys is normalised here on purpose.
    #[must_use]
    pub fn new(revision: u64, author: u32, edits: Vec<Edit>) -> Self {
        let mut batch = Self {
            revision,
            author,
            edits,
        };
        batch.normalize();
        batch
    }

    /// Records one change in a pending batch (`J1.1`).
    ///
    /// Returns `false` and records nothing when `old == new` (`J1.4`). `batch`
    /// is the sub-batch index within the revision (`J2.1`); every change added
    /// to one pending batch normally shares it. `seq` is the current position
    /// and is renumbered by [`commit`](Self::commit).
    pub fn push(
        &mut self,
        position: Coord,
        old: impl Into<BlockName>,
        new: impl Into<BlockName>,
        batch: u32,
    ) -> bool {
        let seq = u32::try_from(self.edits.len()).unwrap_or(u32::MAX);
        let Some(edit) = Edit::record(position, old, new, self.revision, self.author, batch, seq)
        else {
            return false;
        };
        self.edits.push(edit);
        true
    }

    /// Seals the batch at `next_revision` (`J1.2`).
    ///
    /// The caller owns the monotonic world revision and passes the next value;
    /// nothing global is read or bumped here. Every record is stamped with
    /// `next_revision`, the batch author, and its position as `seq`, and any
    /// no-op is dropped (`J1.4`).
    pub fn commit(&mut self, next_revision: u64) {
        self.revision = next_revision;
        self.normalize();
    }

    /// Number of records the next commit will keep.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.edits.len()
    }

    /// Whether the batch holds no committed change.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    /// Drops no-ops and stamps the batch fields onto every record.
    fn normalize(&mut self) {
        let revision = self.revision;
        let author = self.author;
        self.edits.retain(|edit| edit.old != edit.new);
        for (position, edit) in self.edits.iter_mut().enumerate() {
            edit.revision = revision;
            edit.author = author;
            edit.seq = u32::try_from(position).unwrap_or(u32::MAX);
        }
    }
}

/// One record per coordinate: the highest `(revision, batch, seq)` wins.
///
/// Output is sorted by coordinate (`x`, then `y`, then `z`) and is a pure
/// function of the input set, so two arrival orders produce identical bytes.
/// This is the read-side answer to `J2.1`; the log itself is never rewritten.
#[must_use]
pub fn last_write_wins(edits: &[Edit]) -> Vec<Edit> {
    let mut winners: BTreeMap<Coord, &Edit> = BTreeMap::new();
    for edit in edits {
        let key = edit.position();
        let candidate = edit.order_key();
        let wins = winners
            .get(&key)
            .is_none_or(|current| candidate > current.order_key());
        if wins {
            winners.insert(key, edit);
        }
    }
    winners.into_values().cloned().collect()
}

/// Net diff per coordinate: the first `old`, the last `new` (`J3.2`).
///
/// Records are visited in total order, so a coordinate that was changed and
/// then changed back disappears: its net diff is the baseline and `J1.4` says
/// nothing is recorded when `old == new`. The surviving record carries the last
/// write's revision, author, batch, and seq, because that is the write the net
/// value came from. Output is sorted by coordinate.
#[must_use]
pub fn coalesce(edits: &[Edit]) -> Vec<Edit> {
    let mut ordered: Vec<&Edit> = edits.iter().collect();
    ordered.sort();

    let mut net: BTreeMap<Coord, Edit> = BTreeMap::new();
    for edit in ordered {
        match net.get_mut(&edit.position()) {
            Some(existing) => {
                existing.new.clone_from(&edit.new);
                existing.revision = edit.revision;
                existing.author = edit.author;
                existing.batch = edit.batch;
                existing.seq = edit.seq;
            }
            None => {
                net.insert(edit.position(), edit.clone());
            }
        }
    }
    net.into_values()
        .filter(|edit| edit.old != edit.new)
        .collect()
}

/// Whether an edit can be undone against the block that is there now (`J2.3`).
///
/// Undo is allowed only when the block still holds the record's `new` value;
/// anything else is a conflict and is reported, never forced. `current` is
/// `None` when the caller has no value for the coordinate at all, which never
/// counts as a match: the caller maps "no block" to the air name explicitly.
#[must_use]
pub fn can_undo(edit: &Edit, current: Option<&str>) -> bool {
    current.is_some_and(|name| name == edit.new)
}

/// Encodes one batch as one frame (`J3.3`).
///
/// The frame carries the batch revision, the batch author, and every record
/// that is not a no-op. A payload larger than `u32::MAX` bytes is not
/// representable; the length saturates so recovery treats the frame as torn
/// instead of misreading it.
#[must_use]
pub fn encode_batch(batch: &EditBatch) -> Vec<u8> {
    let records: Vec<&Edit> = batch
        .edits
        .iter()
        .filter(|edit| edit.old != edit.new)
        .collect();

    let mut payload = Vec::with_capacity(8 + records.len() * MIN_RECORD_BYTES);
    push_u32(&mut payload, batch.author);
    push_u32(&mut payload, u32_length(records.len()));
    for edit in records {
        push_i32(&mut payload, edit.x);
        push_i32(&mut payload, edit.y);
        push_i32(&mut payload, edit.z);
        push_u32(&mut payload, edit.seq);
        push_u32(&mut payload, edit.author);
        push_u32(&mut payload, edit.batch);
        push_name(&mut payload, &edit.old);
        push_name(&mut payload, &edit.new);
    }

    let revision = batch.revision.to_le_bytes();
    let mut frame = Vec::with_capacity(payload.len() + FRAME_OVERHEAD);
    push_u32(&mut frame, u32_length(payload.len()));
    frame.extend_from_slice(&revision);
    frame.extend_from_slice(&payload);
    let crc = crc32_update(crc32(&revision), &payload);
    push_u32(&mut frame, crc);
    frame
}

/// Decodes every complete frame and truncates a torn tail (`J3.3`).
///
/// The returned offset is the number of bytes that belong to complete frames.
/// When it is smaller than `bytes.len()`, the rest is a partial write: the
/// caller keeps the prefix and appends after the offset. Truncation is not an
/// error here, by design.
///
/// # Errors
///
/// A *complete* frame that fails its crc, its bit widths, or its UTF-8 is
/// rejected: a corrupt byte inside the valid region must never be silently
/// dropped together with everything after it.
pub fn decode_batches(bytes: &[u8]) -> Result<(Vec<EditBatch>, usize), TerrainError> {
    let mut batches = Vec::new();
    let mut offset = 0_usize;
    while let Some(rest) = bytes.get(offset..) {
        if rest.len() < HEADER_BYTES {
            break;
        }
        let Some(length) = read_u32(rest, 0) else {
            break;
        };
        let Ok(length) = usize::try_from(length) else {
            break;
        };
        let Some(frame_length) = length.checked_add(FRAME_OVERHEAD) else {
            break;
        };
        let Some(end) = offset.checked_add(frame_length) else {
            break;
        };
        let Some(frame) = bytes.get(offset..end) else {
            break;
        };
        let Some(revision) = read_u64(frame, 4) else {
            break;
        };
        let crc_at = frame.len() - 4;
        let Some(payload) = frame.get(12..crc_at) else {
            break;
        };
        let Some(expected) = read_u32(frame, crc_at) else {
            break;
        };
        let actual = crc32_update(crc32(&revision.to_le_bytes()), payload);
        if actual != expected {
            return Err(framing_error());
        }
        batches.push(decode_payload(revision, payload)?);
        offset = end;
    }
    Ok((batches, offset))
}

/// Decodes like [`decode_batches`], but reports a torn tail as an error.
///
/// Recovery wants the valid prefix and its offset, which [`decode_batches`]
/// returns. This wrapper exists for callers that need the typed notice
/// [`ErrorKind::JournalTornTail`] with the journal path, for example a load
/// banner. It never truncates silently.
///
/// # Errors
///
/// Returns [`ErrorKind::JournalTornTail`] when `bytes` ends mid-frame, and the
/// same framing errors as [`decode_batches`] for a corrupt complete frame.
pub fn decode_with_report(
    bytes: &[u8],
    path: impl Into<PathBuf>,
) -> Result<(Vec<EditBatch>, usize), TerrainError> {
    let (batches, consumed) = decode_batches(bytes)?;
    if consumed < bytes.len() {
        return Err(TerrainError::new(
            Stage::Persistence,
            ErrorKind::JournalTornTail {
                path: path.into(),
                valid_bytes: u64::try_from(consumed).unwrap_or(u64::MAX),
            },
        ));
    }
    Ok((batches, consumed))
}

/// Group-commit thresholds from `J3.1`.
///
/// The fields are public so a caller can tune them in tests; [`new`](Self::new)
/// is the spec default (64 batches, 250 ms, 10 s). Every method is pure: the
/// caller passes monotonic milliseconds and no clock is read here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommitPolicy {
    /// Flush once this many complete batches are buffered.
    pub max_batches: u32,
    /// Flush once this many monotonic milliseconds have passed since the last
    /// successful flush.
    pub max_age_ms: u64,
    /// Sync once this many monotonic milliseconds have passed since the last
    /// successful sync.
    pub fsync_ms: u64,
}

impl CommitPolicy {
    /// The spec defaults: 64 batches, 250 ms, 10 s.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            max_batches: 64,
            max_age_ms: 250,
            fsync_ms: 10_000,
        }
    }

    /// Whether a flush is due.
    ///
    /// `queued_batches` and `queued_bytes` describe the in-memory buffer;
    /// `since_flush_ms` is monotonic milliseconds since the last successful
    /// flush. A buffer that holds nothing is never due, even when time has
    /// passed. There is no byte threshold in `J3.1`; `queued_bytes` only makes
    /// that emptiness test correct for a caller that counts bytes separately.
    /// No clock is read.
    #[must_use]
    pub const fn should_flush(
        &self,
        queued_batches: u32,
        queued_bytes: u64,
        since_flush_ms: u64,
    ) -> bool {
        let pending = queued_batches > 0 || queued_bytes > 0;
        pending && (queued_batches >= self.max_batches || since_flush_ms >= self.max_age_ms)
    }

    /// Whether the `J3.1` periodic fsync is due.
    ///
    /// `since_sync_ms` is monotonic milliseconds since the last successful
    /// sync. Pause, quit, and a section save must call
    /// [`JournalWriter::flush`] instead of waiting for this threshold. No clock
    /// is read.
    #[must_use]
    pub const fn should_sync(&self, since_sync_ms: u64) -> bool {
        since_sync_ms >= self.fsync_ms
    }
}

impl Default for CommitPolicy {
    fn default() -> Self {
        Self::new()
    }
}

/// The byte destination a [`JournalWriter`] appends to.
///
/// Implementations append every byte or return an error. They never seek,
/// truncate, or reopen: the IO thread owns the file, its recovery, and its
/// position. The trait is path-agnostic on purpose, so a sink failure is
/// reported as [`ErrorKind::IoError`] with the method name and a placeholder
/// path.
pub trait JournalSink {
    /// Appends all of `bytes`.
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()>;

    /// Makes every previously written byte durable.
    fn sync_data(&mut self) -> std::io::Result<()>;
}

/// Appends at the file's current position and syncs it.
///
/// The caller opens the journal for append (or positions it after the
/// recovered prefix) and truncates a torn tail before constructing the writer.
impl JournalSink for std::fs::File {
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        <std::fs::File as std::io::Write>::write_all(self, bytes)
    }

    fn sync_data(&mut self) -> std::io::Result<()> {
        std::fs::File::sync_data(self)
    }
}

/// An in-memory sink, so tests and callers can inspect the exact bytes.
impl JournalSink for Vec<u8> {
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.extend_from_slice(bytes);
        Ok(())
    }

    fn sync_data(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// What [`JournalWriter::flush_if_due`] did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlushOutcome {
    /// Nothing was due; the buffer is untouched.
    Idle,
    /// The buffer was written in one `write_all`; no sync was due.
    Flushed,
    /// The buffer was written and `sync_data` completed.
    FlushedAndSynced,
}

/// Buffers encoded frames and commits them in groups (`J3.1`).
///
/// The writer reads no clock and opens no file. Push each committed
/// [`EditBatch`] with [`push`](Self::push); the frame is encoded immediately
/// but stays in memory until [`flush_if_due`](Self::flush_if_due) or
/// [`flush`](Self::flush) writes it. The IO thread passes its monotonic
/// millisecond count with the same origin as writer creation, which starts at
/// zero.
///
/// # Errors
///
/// Every sink failure is [`ErrorKind::IoError`](crate::error::ErrorKind::IoError)
/// with `op` set to the sink method that failed (`"write_all"` or
/// `"sync_data"`) and the OS error code, or `0` when the sink reported a
/// non-OS error.
///
/// A failed `write_all` leaves the buffer and `pending_batches` untouched, so a
/// retry cannot drop records. That guarantee assumes the sink did not append a
/// partial frame before failing; a real file can, so the IO thread must treat a
/// write failure as session-fatal and recover the tail on reopen rather than
/// blindly retrying. A failed `sync_data` happens after the bytes were written:
/// the pending count is already cleared and only durability is missing.
pub struct JournalWriter<S: JournalSink> {
    sink: S,
    policy: CommitPolicy,
    buffer: Vec<u8>,
    pending_batches: u32,
    pending_bytes: u64,
    batches_written: u64,
    bytes_written: u64,
    syncs: u64,
    /// Last `now_ms` supplied by the caller, clamped monotonic.
    clock_ms: u64,
    last_flush_ms: u64,
    last_sync_ms: u64,
}

impl<S: JournalSink> JournalWriter<S> {
    /// Wraps `sink` with the spec policy (or a caller-tuned one).
    #[must_use]
    pub const fn new(sink: S, policy: CommitPolicy) -> Self {
        Self {
            sink,
            policy,
            buffer: Vec::new(),
            pending_batches: 0,
            pending_bytes: 0,
            batches_written: 0,
            bytes_written: 0,
            syncs: 0,
            clock_ms: 0,
            last_flush_ms: 0,
            last_sync_ms: 0,
        }
    }

    /// Encodes one batch into the buffer without writing it.
    ///
    /// The frame is built now, so a later mutation of `batch` cannot change
    /// what is written. Nothing is flushed until a policy threshold or an
    /// explicit [`flush`](Self::flush).
    pub fn push(&mut self, batch: &EditBatch) -> Result<(), TerrainError> {
        let frame = encode_batch(batch);
        self.pending_bytes = self
            .pending_bytes
            .saturating_add(u64::try_from(frame.len()).unwrap_or(u64::MAX));
        self.buffer.extend_from_slice(&frame);
        self.pending_batches = self.pending_batches.saturating_add(1);
        Ok(())
    }

    /// Flushes and/or syncs only when the policy says so at `now_ms`.
    ///
    /// `now_ms` is caller-supplied monotonic milliseconds since writer
    /// creation; this method reads no clock. A value below the previous call is
    /// clamped, so a misbehaving caller cannot make the counters go backwards.
    ///
    /// The fsync threshold is evaluated as part of a flush, so a tick with
    /// nothing to flush returns [`FlushOutcome::Idle`] without syncing. A
    /// caller enforcing a bare 10 s barrier while the buffer is empty calls
    /// [`flush`](Self::flush) instead.
    ///
    /// # Errors
    ///
    /// See [`JournalWriter`]. A write failure leaves the buffer pending; a sync
    /// failure leaves the bytes written but not durable.
    pub fn flush_if_due(&mut self, now_ms: u64) -> Result<FlushOutcome, TerrainError> {
        self.clock_ms = self.clock_ms.max(now_ms);
        let since_flush_ms = self.clock_ms.saturating_sub(self.last_flush_ms);
        if !self
            .policy
            .should_flush(self.pending_batches, self.pending_bytes, since_flush_ms)
        {
            return Ok(FlushOutcome::Idle);
        }

        self.write_pending()?;
        let since_sync_ms = self.clock_ms.saturating_sub(self.last_sync_ms);
        if self.policy.should_sync(since_sync_ms) {
            self.sync()?;
            Ok(FlushOutcome::FlushedAndSynced)
        } else {
            Ok(FlushOutcome::Flushed)
        }
    }

    /// Writes the buffer and syncs it unconditionally.
    ///
    /// This is the pause, quit, and before-section-save barrier. It syncs even
    /// when the buffer is empty, because earlier flushes may have left written
    /// bytes unsynced.
    ///
    /// # Errors
    ///
    /// See [`JournalWriter`]. A write failure leaves the buffer pending; a sync
    /// failure leaves the bytes written but not durable.
    pub fn flush(&mut self) -> Result<(), TerrainError> {
        self.write_pending()?;
        self.sync()
    }

    /// Consumes the writer and returns the sink.
    ///
    /// This does not flush: pending frames are discarded with the writer.
    #[must_use]
    pub fn into_sink(self) -> S {
        self.sink
    }

    /// Batches successfully written to the sink, whether or not yet synced.
    #[must_use]
    pub const fn batches_written(&self) -> u64 {
        self.batches_written
    }

    /// Encoded bytes successfully written to the sink, whether or not yet
    /// synced.
    #[must_use]
    pub const fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    /// Successful `sync_data` calls.
    #[must_use]
    pub const fn syncs(&self) -> u64 {
        self.syncs
    }

    /// Batches buffered but not yet written. A failed write leaves this intact.
    #[must_use]
    pub const fn pending_batches(&self) -> u32 {
        self.pending_batches
    }

    /// Encoded bytes buffered but not yet written.
    #[must_use]
    pub const fn pending_bytes(&self) -> u64 {
        self.pending_bytes
    }

    /// Writes the buffer in one call and updates the written counters.
    fn write_pending(&mut self) -> Result<(), TerrainError> {
        if self.pending_batches == 0 {
            return Ok(());
        }
        self.sink
            .write_all(&self.buffer)
            .map_err(|error| sink_error("write_all", &error))?;
        self.batches_written = self
            .batches_written
            .saturating_add(u64::from(self.pending_batches));
        self.bytes_written = self.bytes_written.saturating_add(self.pending_bytes);
        self.buffer.clear();
        self.pending_batches = 0;
        self.pending_bytes = 0;
        self.last_flush_ms = self.clock_ms;
        Ok(())
    }

    /// Syncs the sink and updates the sync counters.
    fn sync(&mut self) -> Result<(), TerrainError> {
        self.sink
            .sync_data()
            .map_err(|error| sink_error("sync_data", &error))?;
        self.syncs = self.syncs.saturating_add(1);
        self.last_sync_ms = self.clock_ms;
        Ok(())
    }
}

impl<S: JournalSink> fmt::Debug for JournalWriter<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JournalWriter")
            .field("policy", &self.policy)
            .field("pending_batches", &self.pending_batches)
            .field("pending_bytes", &self.pending_bytes)
            .field("batches_written", &self.batches_written)
            .field("bytes_written", &self.bytes_written)
            .field("syncs", &self.syncs)
            .finish_non_exhaustive()
    }
}

/// Wraps a sink failure as a persistence error.
///
/// The sink has no path (`J3.1` keeps the file on the IO thread), so the error
/// carries the placeholder path `"<journal>"` and the operation name.
fn sink_error(op: &'static str, error: &std::io::Error) -> TerrainError {
    TerrainError::new(
        Stage::Persistence,
        ErrorKind::IoError {
            op,
            path: PathBuf::from("<journal>"),
            os_error: error.raw_os_error().unwrap_or(0),
        },
    )
}

/// Decodes the payload of one frame.
fn decode_payload(revision: u64, payload: &[u8]) -> Result<EditBatch, TerrainError> {
    let mut reader = Reader::new(payload);
    let author = reader.u32().ok_or_else(framing_error)?;
    let count = reader.u32().ok_or_else(framing_error)?;
    let Ok(count) = usize::try_from(count) else {
        return Err(framing_error());
    };
    let Some(minimum) = count.checked_mul(MIN_RECORD_BYTES) else {
        return Err(framing_error());
    };
    if reader.remaining() < minimum {
        return Err(framing_error());
    }

    let mut edits = Vec::with_capacity(count);
    for _ in 0..count {
        let x = reader.i32().ok_or_else(framing_error)?;
        let y = reader.i32().ok_or_else(framing_error)?;
        let z = reader.i32().ok_or_else(framing_error)?;
        let seq = reader.u32().ok_or_else(framing_error)?;
        let edit_author = reader.u32().ok_or_else(framing_error)?;
        let batch = reader.u32().ok_or_else(framing_error)?;
        let old = reader.name().ok_or_else(framing_error)?;
        let new = reader.name().ok_or_else(framing_error)?;
        edits.push(Edit {
            x,
            y,
            z,
            old,
            new,
            revision,
            author: edit_author,
            batch,
            seq,
        });
    }
    if !reader.is_empty() {
        return Err(framing_error());
    }
    Ok(EditBatch {
        revision,
        author,
        edits,
    })
}

/// A cursor over a byte slice that never indexes out of bounds.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    /// Starts at the first byte.
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    /// Bytes not yet consumed.
    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.at)
    }

    /// Whether the cursor reached the end.
    fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// Consumes `length` bytes, or `None` when the slice is too short.
    fn take(&mut self, length: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(length)?;
        let slice = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(slice)
    }

    /// Consumes one little-endian `u32`.
    fn u32(&mut self) -> Option<u32> {
        let chunk: [u8; 4] = self.take(4)?.try_into().ok()?;
        Some(u32::from_le_bytes(chunk))
    }

    /// Consumes one little-endian `i32`.
    fn i32(&mut self) -> Option<i32> {
        let chunk: [u8; 4] = self.take(4)?.try_into().ok()?;
        Some(i32::from_le_bytes(chunk))
    }

    /// Consumes a length-prefixed UTF-8 name.
    fn name(&mut self) -> Option<BlockName> {
        let length = usize::try_from(self.u32()?).ok()?;
        let bytes = self.take(length)?;
        std::str::from_utf8(bytes).ok().map(str::to_owned)
    }
}

/// Reads a little-endian `u32` at `at`, or `None` when out of bounds.
fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let end = at.checked_add(4)?;
    let chunk: [u8; 4] = bytes.get(at..end)?.try_into().ok()?;
    Some(u32::from_le_bytes(chunk))
}

/// Reads a little-endian `u64` at `at`, or `None` when out of bounds.
fn read_u64(bytes: &[u8], at: usize) -> Option<u64> {
    let end = at.checked_add(8)?;
    let chunk: [u8; 8] = bytes.get(at..end)?.try_into().ok()?;
    Some(u64::from_le_bytes(chunk))
}

/// The one error shape for an impossible frame.
fn framing_error() -> TerrainError {
    TerrainError::new(
        Stage::Persistence,
        ErrorKind::InvariantViolated {
            check: "journal_framing",
            coord: None,
        },
    )
}

/// Appends an `i32` little-endian.
fn push_i32(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// Appends a `u32` little-endian.
fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// Appends a length-prefixed name.
fn push_name(out: &mut Vec<u8>, name: &str) {
    push_u32(out, u32_length(name.len()));
    out.extend_from_slice(name.as_bytes());
}

/// Narrows a length for the wire, saturating rather than panicking.
fn u32_length(length: usize) -> u32 {
    u32::try_from(length).unwrap_or(u32::MAX)
}

/// The reflected IEEE polynomial (`0xEDB8_8320`).
const CRC32_POLYNOMIAL: u32 = 0xEDB8_8320;

/// Byte-wise crc32 lookup table. `index < 256` makes the cast exact.
#[allow(clippy::cast_possible_truncation)]
const fn crc32_table() -> [u32; 256] {
    let mut table = [0_u32; 256];
    let mut index = 0_usize;
    while index < 256 {
        let mut crc = index as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 == 0 {
                crc >> 1
            } else {
                (crc >> 1) ^ CRC32_POLYNOMIAL
            };
            bit += 1;
        }
        table[index] = crc;
        index += 1;
    }
    table
}

static CRC32_TABLE: [u32; 256] = crc32_table();

/// IEEE crc32, reflected, initial value all ones, final inversion.
///
/// Check value: `crc32(b"123456789") == 0xCBF4_3926`.
#[must_use]
pub fn crc32(bytes: &[u8]) -> u32 {
    crc32_update(0, bytes)
}

/// Continues a crc32, so `crc32_update(crc32(a), b) == crc32(a ++ b)`.
#[allow(clippy::cast_possible_truncation)] // masked to eight bits
fn crc32_update(previous: u32, bytes: &[u8]) -> u32 {
    let mut state = !previous;
    for &byte in bytes {
        let index = ((state ^ u32::from(byte)) & 0xff) as usize;
        state = CRC32_TABLE[index] ^ (state >> 8);
    }
    !state
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn record(position: Coord, change: (&str, &str), order: (u64, u32, u32)) -> Edit {
        Edit::record(position, change.0, change.1, order.0, 7, order.1, order.2)
            .expect("test records always change the block")
    }

    /// Bypasses [`Edit::record`] so a test can hand a no-op to a filter.
    fn raw(position: Coord, old: &str, new: &str, order: (u64, u32, u32)) -> Edit {
        Edit {
            x: position[0],
            y: position[1],
            z: position[2],
            old: old.to_owned(),
            new: new.to_owned(),
            revision: order.0,
            author: 7,
            batch: order.1,
            seq: order.2,
        }
    }

    fn sample_batch() -> EditBatch {
        let mut batch = EditBatch::new(0, 7, Vec::new());
        assert!(batch.push([1, 2, 3], "core:stone", "core:air", 0));
        assert!(batch.push([4, 5, 6], "core:air", "modpacks:mythril_ore", 0));
        batch.commit(3);
        batch
    }

    fn batch_at(revision: u64) -> EditBatch {
        let mut batch = EditBatch::new(revision, 7, Vec::new());
        assert!(batch.push([1, 2, 3], "core:stone", "core:air", 0));
        batch.commit(revision);
        batch
    }

    /// A sink that can fail exactly once, to prove the retry path.
    #[derive(Debug, Default)]
    struct FlakySink {
        bytes: Vec<u8>,
        fail_write_once: bool,
        fail_sync_once: bool,
        syncs: u32,
    }

    impl JournalSink for FlakySink {
        fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
            if self.fail_write_once {
                self.fail_write_once = false;
                return Err(std::io::Error::from_raw_os_error(5));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(())
        }

        fn sync_data(&mut self) -> std::io::Result<()> {
            if self.fail_sync_once {
                self.fail_sync_once = false;
                return Err(std::io::Error::from_raw_os_error(6));
            }
            self.syncs += 1;
            Ok(())
        }
    }

    #[test]
    fn policy_defaults_match_the_spec() {
        let policy = CommitPolicy::new();
        assert_eq!(policy, CommitPolicy::default());
        assert_eq!(policy.max_batches, 64);
        assert_eq!(policy.max_age_ms, 250);
        assert_eq!(policy.fsync_ms, 10_000);

        assert!(!policy.should_flush(63, 1_024, 249));
        assert!(policy.should_flush(64, 1_024, 0));
        assert!(policy.should_flush(1, 16, 250));
        assert!(!policy.should_flush(0, 0, 10_000));
        assert!(!policy.should_sync(9_999));
        assert!(policy.should_sync(10_000));
    }

    #[test]
    fn writer_waits_for_sixty_four_batches() {
        let mut writer = JournalWriter::new(Vec::new(), CommitPolicy::new());
        for revision in 1..=63 {
            writer.push(&batch_at(revision)).unwrap();
        }
        assert_eq!(writer.flush_if_due(0).unwrap(), FlushOutcome::Idle);
        assert_eq!(writer.pending_batches(), 63);
        assert_eq!(writer.batches_written(), 0);

        writer.push(&batch_at(64)).unwrap();
        assert_eq!(writer.flush_if_due(0).unwrap(), FlushOutcome::Flushed);
        assert_eq!(writer.pending_batches(), 0);
        assert_eq!(writer.batches_written(), 64);
        assert!(writer.bytes_written() > 0);
    }

    #[test]
    fn writer_flushes_a_lone_batch_after_its_age() {
        let mut writer = JournalWriter::new(Vec::new(), CommitPolicy::new());
        writer.push(&sample_batch()).unwrap();
        assert_eq!(writer.flush_if_due(249).unwrap(), FlushOutcome::Idle);
        assert_eq!(writer.pending_batches(), 1);

        assert_eq!(writer.flush_if_due(250).unwrap(), FlushOutcome::Flushed);
        assert_eq!(writer.pending_batches(), 0);
        assert_eq!(writer.batches_written(), 1);
    }

    #[test]
    fn writer_syncs_every_ten_seconds_and_flush_always_syncs() {
        let mut writer = JournalWriter::new(Vec::new(), CommitPolicy::new());
        writer.push(&sample_batch()).unwrap();
        assert_eq!(writer.flush_if_due(250).unwrap(), FlushOutcome::Flushed);
        assert_eq!(writer.syncs(), 0);

        writer.push(&batch_at(4)).unwrap();
        assert_eq!(
            writer.flush_if_due(10_000).unwrap(),
            FlushOutcome::FlushedAndSynced
        );
        assert_eq!(writer.syncs(), 1);

        writer.flush().unwrap();
        assert_eq!(writer.syncs(), 2, "flush syncs even with nothing pending");
    }

    #[test]
    fn writer_round_trips_every_batch_in_order() {
        let batches: Vec<EditBatch> = (1..=5).map(batch_at).collect();
        let expected_bytes: usize = batches.iter().map(|batch| encode_batch(batch).len()).sum();

        let mut writer = JournalWriter::new(Vec::new(), CommitPolicy::new());
        for batch in &batches {
            writer.push(batch).unwrap();
        }
        writer.flush().unwrap();
        assert_eq!(writer.pending_batches(), 0);
        assert_eq!(writer.batches_written(), 5);
        assert_eq!(
            writer.bytes_written(),
            u64::try_from(expected_bytes).unwrap()
        );

        let sink = writer.into_sink();
        let (decoded, consumed) = decode_batches(&sink).unwrap();
        assert_eq!(consumed, sink.len(), "every byte belongs to a frame");
        assert_eq!(decoded.len(), batches.len());
        for (left, right) in decoded.iter().zip(&batches) {
            assert_eq!(left.revision, right.revision);
            assert_eq!(left.author, right.author);
            assert_eq!(left.edits.len(), right.edits.len());
            for (left, right) in left.edits.iter().zip(&right.edits) {
                assert!(left.same_record(right), "{left:?} != {right:?}");
            }
        }
    }

    #[test]
    fn torn_tail_written_to_a_vec_sink_is_truncated() {
        let first = encode_batch(&sample_batch());
        let second = encode_batch(&batch_at(4));
        let mut sink: Vec<u8> = Vec::new();
        sink.extend_from_slice(&first);
        sink.extend_from_slice(&second[..second.len() - 3]);

        let (decoded, consumed) = decode_batches(&sink).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].revision, 3);
        assert_eq!(consumed, first.len());
        assert!(consumed < sink.len());
    }

    #[test]
    fn failed_write_keeps_pending_batches_for_retry() {
        let sink = FlakySink {
            fail_write_once: true,
            ..FlakySink::default()
        };
        let mut writer = JournalWriter::new(sink, CommitPolicy::new());
        writer.push(&sample_batch()).unwrap();
        writer.push(&batch_at(4)).unwrap();

        let error = writer.flush_if_due(250).unwrap_err();
        assert_eq!(error.stage, Stage::Persistence);
        match error.kind {
            ErrorKind::IoError { op, path, os_error } => {
                assert_eq!(op, "write_all");
                assert_eq!(path, PathBuf::from("<journal>"));
                assert_eq!(os_error, 5);
            }
            other => panic!("expected an io error, got {other:?}"),
        }
        assert_eq!(writer.pending_batches(), 2);
        assert_eq!(writer.batches_written(), 0);
        assert_eq!(writer.bytes_written(), 0);

        assert_eq!(writer.flush_if_due(250).unwrap(), FlushOutcome::Flushed);
        assert_eq!(writer.pending_batches(), 0);
        assert_eq!(writer.batches_written(), 2);

        let sink = writer.into_sink();
        let (decoded, consumed) = decode_batches(&sink.bytes).unwrap();
        assert_eq!(consumed, sink.bytes.len());
        assert_eq!(decoded.len(), 2);
    }

    #[test]
    fn failed_sync_keeps_the_written_bytes_and_retries() {
        let sink = FlakySink {
            fail_sync_once: true,
            ..FlakySink::default()
        };
        let mut writer = JournalWriter::new(sink, CommitPolicy::new());
        writer.push(&sample_batch()).unwrap();

        let error = writer.flush().unwrap_err();
        match error.kind {
            ErrorKind::IoError { op, os_error, .. } => {
                assert_eq!(op, "sync_data");
                assert_eq!(os_error, 6);
            }
            other => panic!("expected an io error, got {other:?}"),
        }
        assert_eq!(
            writer.pending_batches(),
            0,
            "the bytes were already written"
        );
        assert_eq!(writer.batches_written(), 1);
        assert_eq!(writer.syncs(), 0);

        writer.flush().unwrap();
        assert_eq!(writer.syncs(), 1);
        let sink = writer.into_sink();
        assert_eq!(sink.syncs, 1, "the failed attempt is not counted");
        let (decoded, consumed) = decode_batches(&sink.bytes).unwrap();
        assert_eq!(consumed, sink.bytes.len());
        assert_eq!(decoded.len(), 1);
    }

    #[test]
    fn record_round_trips_through_framing() {
        let batch = sample_batch();
        let bytes = encode_batch(&batch);
        let (decoded, consumed) = decode_batches(&bytes).unwrap();

        assert_eq!(consumed, bytes.len());
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].revision, 3);
        assert_eq!(decoded[0].author, 7);
        assert_eq!(decoded[0].edits.len(), 2);
        for (left, right) in decoded[0].edits.iter().zip(&batch.edits) {
            assert!(left.same_record(right), "{left:?} != {right:?}");
        }
        assert_eq!(decoded[0].edits[0].seq, 0);
        assert_eq!(decoded[0].edits[1].seq, 1);

        let mut two = bytes.clone();
        two.extend_from_slice(&encode_batch(&sample_batch()));
        let (decoded, consumed) = decode_batches(&two).unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!(consumed, two.len());
    }

    #[test]
    fn torn_tail_is_truncated_and_reported() {
        let first = encode_batch(&sample_batch());
        let second = encode_batch(&sample_batch());
        let mut bytes = first.clone();
        bytes.extend_from_slice(&second[..second.len() - 5]);

        let (decoded, consumed) = decode_batches(&bytes).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(consumed, first.len());

        let error = decode_with_report(&bytes, "journal/0000.log").unwrap_err();
        assert_eq!(error.stage, Stage::Persistence);
        match error.kind {
            ErrorKind::JournalTornTail { path, valid_bytes } => {
                assert_eq!(path, PathBuf::from("journal/0000.log"));
                assert_eq!(valid_bytes, u64::try_from(first.len()).unwrap());
            }
            other => panic!("expected a torn tail, got {other:?}"),
        }

        // A tail that stops inside the length/revision header consumes nothing.
        let (decoded, consumed) = decode_batches(&second[..7]).unwrap();
        assert!(decoded.is_empty());
        assert_eq!(consumed, 0);
        assert!(decode_with_report(&second[..7], "j").is_err());
    }

    #[test]
    fn crc_rejects_a_flipped_bit() {
        let bytes = encode_batch(&sample_batch());
        let mut corrupt = bytes.clone();
        let index = corrupt.len() - 8;
        corrupt[index] ^= 0x01;

        let error = decode_batches(&corrupt).unwrap_err();
        assert!(
            matches!(error.kind, ErrorKind::InvariantViolated { .. }),
            "flipped payload bit must not look like a torn tail"
        );
        assert_eq!(decode_batches(&bytes).unwrap().1, bytes.len());
    }

    #[test]
    fn crc32_matches_the_ieee_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32_update(crc32(b"1234"), b"56789"), crc32(b"123456789"));
    }

    #[test]
    fn last_write_wins_uses_revision_batch_seq() {
        let edits = vec![
            record([1, 0, 1], ("core:stone", "core:dirt"), (4, 0, 0)),
            record([1, 0, 1], ("core:dirt", "core:grass"), (5, 0, 0)),
            record([1, 0, 1], ("core:grass", "core:sand"), (5, 1, 0)),
            record([0, 0, 0], ("core:stone", "core:air"), (1, 0, 0)),
            record([2, 0, 2], ("core:stone", "core:gravel"), (9, 0, 0)),
            record([2, 0, 2], ("core:gravel", "core:clay"), (9, 0, 3)),
            record([2, 0, 2], ("core:clay", "core:moss"), (8, 7, 7)),
        ];

        let winners = last_write_wins(&edits);
        assert_eq!(
            winners.iter().map(Edit::position).collect::<Vec<_>>(),
            vec![[0, 0, 0], [1, 0, 1], [2, 0, 2]]
        );
        assert_eq!(winners[0].new, "core:air");
        assert_eq!(winners[1].new, "core:sand");
        assert_eq!(winners[1].order_key(), (5, 1, 0));
        assert_eq!(winners[2].new, "core:clay");

        let mut reversed = edits;
        reversed.reverse();
        let shuffled_winners = last_write_wins(&reversed);
        assert_eq!(shuffled_winners.len(), 3);
        for (left, right) in shuffled_winners.iter().zip(&winners) {
            assert!(left.same_record(right));
        }
    }

    #[test]
    fn order_is_total_and_stable_under_shuffle() {
        let records = [
            record([0, 0, 0], ("core:a", "core:b"), (2, 0, 0)),
            record([1, 0, 0], ("core:a", "core:b"), (1, 5, 0)),
            record([2, 0, 0], ("core:a", "core:b"), (1, 0, 7)),
            record([3, 0, 0], ("core:a", "core:b"), (1, 0, 2)),
            record([4, 0, 0], ("core:a", "core:b"), (3, 0, 0)),
        ];
        let expected = [3, 2, 1, 0, 4];

        for permutation in [
            [0, 1, 2, 3, 4],
            [4, 3, 2, 1, 0],
            [2, 4, 0, 3, 1],
            [1, 3, 0, 4, 2],
        ] {
            let mut shuffled: Vec<Edit> = permutation
                .iter()
                .map(|&index| records[index].clone())
                .collect();
            shuffled.sort();
            let order: Vec<Coord> = shuffled.iter().map(Edit::position).collect();
            let want: Vec<Coord> = expected
                .iter()
                .map(|&index| records[index].position())
                .collect();
            assert_eq!(order, want);
            assert!(shuffled.windows(2).all(|pair| pair[0] <= pair[1]));
        }

        // The order is on the key, not the payload: equal keys compare equal
        // and a stable sort keeps them in log order.
        let first = record([0, 0, 0], ("core:a", "core:b"), (1, 1, 1));
        let second = record([9, 9, 9], ("core:c", "core:d"), (1, 1, 1));
        assert_eq!(first.cmp(&second), Ordering::Equal);
        assert_eq!(first, second);
        assert!(!first.same_record(&second));
        let mut pair = [second.clone(), first];
        pair.sort();
        assert_eq!(pair[0].position(), second.position());
    }

    #[test]
    fn no_record_when_old_equals_new() {
        let mut batch = EditBatch::new(1, 0, Vec::new());
        assert!(!batch.push([0, 0, 0], "core:stone", "core:stone", 0));
        assert!(batch.is_empty());

        // Block to air and air to block are both real changes (`J1.4`).
        assert!(batch.push([0, 0, 0], "core:stone", "core:air", 0));
        assert!(batch.push([1, 0, 0], "core:air", "core:stone", 0));
        batch.commit(2);
        assert_eq!(batch.len(), 2);
        assert!(batch.edits.iter().all(|edit| edit.revision == 2));

        // The constructor drops a no-op handed in directly.
        let noisy = EditBatch::new(
            2,
            0,
            vec![
                raw([0, 0, 0], "core:dirt", "core:dirt", (1, 0, 0)),
                record([1, 0, 0], ("core:dirt", "core:air"), (1, 0, 1)),
            ],
        );
        assert_eq!(noisy.len(), 1);
        assert_eq!(noisy.edits[0].seq, 0);

        // The encoder refuses to write a no-op even from a hand-built batch.
        let hand_built = EditBatch {
            revision: 1,
            author: 0,
            edits: vec![
                raw([0, 0, 0], "core:air", "core:air", (1, 0, 0)),
                record([1, 0, 0], ("core:air", "core:stone"), (1, 0, 1)),
            ],
        };
        let (decoded, _) = decode_batches(&encode_batch(&hand_built)).unwrap();
        assert_eq!(decoded[0].edits.len(), 1);
    }

    #[test]
    fn undo_requires_the_new_value_to_still_be_there() {
        let placed = record([0, 0, 0], ("core:air", "core:stone"), (2, 0, 0));
        assert!(can_undo(&placed, Some("core:stone")));
        assert!(!can_undo(&placed, Some("core:dirt")));
        assert!(!can_undo(&placed, None));

        let removed = record([0, 0, 0], ("core:stone", "core:air"), (3, 0, 0));
        assert!(can_undo(&removed, Some("core:air")));
        assert!(!can_undo(&removed, None));
    }

    #[test]
    fn unknown_names_round_trip() {
        let old = "other_pack:resonance_crystal";
        let new = "unknown_pack:air";
        let mut batch = EditBatch::new(11, 0, Vec::new());
        assert!(batch.push([-7, 300, 12], old, new, 0));
        batch.commit(12);

        let bytes = encode_batch(&batch);
        let (decoded, _) = decode_batches(&bytes).unwrap();
        let edit = &decoded[0].edits[0];
        assert_eq!(edit.old, old);
        assert_eq!(edit.new, new);
        assert_eq!(edit.old.as_bytes(), old.as_bytes());
        assert_eq!(edit.new.as_bytes(), new.as_bytes());
        assert_eq!(edit.position(), [-7, 300, 12]);
    }

    #[test]
    fn coalesce_keeps_first_old_and_last_new() {
        let edits = vec![
            record([1, 0, 1], ("core:stone", "core:dirt"), (2, 0, 0)),
            record([1, 0, 1], ("core:dirt", "core:grass"), (3, 0, 0)),
            record([1, 0, 1], ("core:grass", "core:sand"), (4, 0, 0)),
            record([0, 0, 0], ("core:stone", "core:air"), (5, 0, 0)),
            record([0, 0, 0], ("core:air", "core:stone"), (6, 0, 0)),
        ];

        let net = coalesce(&edits);
        assert_eq!(net.len(), 1, "change-and-revert has no net diff");
        assert_eq!(net[0].position(), [1, 0, 1]);
        assert_eq!(net[0].old, "core:stone");
        assert_eq!(net[0].new, "core:sand");
        assert_eq!(net[0].order_key(), (4, 0, 0));

        let mut reversed = edits;
        reversed.reverse();
        assert_eq!(
            coalesce(&reversed)
                .iter()
                .map(|edit| (edit.position(), edit.old.clone(), edit.new.clone()))
                .collect::<Vec<_>>(),
            vec![([1, 0, 1], "core:stone".to_owned(), "core:sand".to_owned())]
        );
    }

    #[test]
    fn malformed_bytes_are_errors_not_panics() {
        // Empty input is a valid empty log, not a torn tail.
        let (empty, consumed) = decode_batches(&[]).unwrap();
        assert!(empty.is_empty());
        assert_eq!(consumed, 0);
        assert!(decode_with_report(&[], "j").is_ok());

        // A complete frame whose payload cannot be read is rejected.
        assert!(decode_batches(&[0_u8; 20]).is_err());

        // A huge length is a torn tail, not an allocation.
        let mut huge = 0xFFFF_FFFF_u32.to_le_bytes().to_vec();
        huge.extend_from_slice(&[0_u8; 8]);
        let (batches, consumed) = decode_batches(&huge).unwrap();
        assert!(batches.is_empty());
        assert_eq!(consumed, 0);

        // Every prefix of a good frame decodes to a prefix or nothing.
        let bytes = encode_batch(&sample_batch());
        for length in 0..bytes.len() {
            let (_, consumed) = decode_batches(&bytes[..length]).unwrap();
            assert!(consumed <= length);
        }
    }
}
