//! Edit records and the framed wire format (`J1`, `J3.3`).

use std::cmp::Ordering;
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
/// read path is [`last_write_wins`](crate::editlog::last_write_wins) and
/// [`coalesce`](crate::editlog::coalesce). Coordinates are world
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
/// care about coordinates key by [`Edit::position`] (as
/// [`last_write_wins`](crate::editlog::last_write_wins) and
/// [`coalesce`](crate::editlog::coalesce) do). Equal keys in one log are an invariant violation
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
    use crate::editlog::test_support::{batch_at, raw, record, sample_batch};
    use crate::error::{ErrorKind, Stage};

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

    #[test]
    fn frame_encoding_is_pinned() {
        // Captured from the pre-split tree; any change to the wire format or
        // to `sample_batch` must be deliberate.
        let bytes = encode_batch(&sample_batch());
        assert_eq!(
            bytes,
            vec![
                118, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0,
                0, 3, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 10, 0, 0, 0, 99, 111, 114, 101,
                58, 115, 116, 111, 110, 101, 8, 0, 0, 0, 99, 111, 114, 101, 58, 97, 105, 114, 4, 0,
                0, 0, 5, 0, 0, 0, 6, 0, 0, 0, 1, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0, 0, 99,
                111, 114, 101, 58, 97, 105, 114, 20, 0, 0, 0, 109, 111, 100, 112, 97, 99, 107, 115,
                58, 109, 121, 116, 104, 114, 105, 108, 95, 111, 114, 101, 77, 47, 61, 67,
            ]
        );
    }
}
