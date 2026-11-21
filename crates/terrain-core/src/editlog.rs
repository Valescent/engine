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
//!
//! # Structure
//!
//! `frame` holds the record types and wire encoding, `log` the group-commit
//! writer, and `replay` the read-side order helpers. Every item is re-exported
//! here, so `editlog::<item>` paths are unchanged.

mod frame;
mod log;
mod replay;
#[cfg(test)]
mod test_support;

pub use frame::{
    BlockName, Edit, EditBatch, crc32, decode_batches, decode_with_report, encode_batch,
};
pub use log::{CommitPolicy, FlushOutcome, JournalSink, JournalWriter};
pub use replay::{can_undo, coalesce, last_write_wins};
