//! The append-only journal writer and its group-commit policy (`J3.1`).

use std::fmt;
use std::path::PathBuf;

use crate::error::{ErrorKind, Stage, TerrainError};

use super::frame::{EditBatch, encode_batch};

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
/// Every sink failure is [`ErrorKind::IoError`]
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::editlog::frame::{decode_batches, encode_batch};
    use crate::editlog::test_support::{batch_at, sample_batch};
    use crate::error::{ErrorKind, Stage};

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
}
