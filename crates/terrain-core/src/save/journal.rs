//! The edit journal: opening, replay, and torn-tail recovery.

use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom};
use std::path::Path;

use crate::editlog::{self, CommitPolicy, EditBatch, JournalWriter};
use crate::error::TerrainError;

use super::io_error;

/// Directory holding append-only journal frames.
pub(crate) const JOURNAL_DIR: &str = "journal";
/// First journal segment. A new segment starts only when a save is interrupted.
pub(crate) const JOURNAL_NAME: &str = "0001.journal";

/// The recovered journal, positioned after its last complete frame.
pub(crate) struct OpenedJournal {
    /// Every committed batch in the recovered prefix, in file order.
    pub recovered: Vec<EditBatch>,
    /// Appends new batches after the recovered prefix.
    pub writer: JournalWriter<File>,
}

impl std::fmt::Debug for OpenedJournal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenedJournal")
            .field("recovered", &self.recovered.len())
            .field("writer", &self.writer)
            .finish()
    }
}

/// Opens (or creates) the journal, recovering and truncating a torn tail.
///
/// The valid prefix is decoded with `editlog::decode_batches`; when the file
/// ends mid-frame the file is truncated to the recovered offset before the
/// writer is constructed, which is the [`JournalWriter`] contract.
///
/// # Errors
///
/// [`crate::error::ErrorKind::IoError`] for a filesystem failure and the framing errors from
/// `editlog::decode_batches` for a corrupt complete frame.
pub(crate) fn open_journal(root: impl AsRef<Path>) -> Result<OpenedJournal, TerrainError> {
    let directory = root.as_ref().join(JOURNAL_DIR);
    fs::create_dir_all(&directory)
        .map_err(|error| io_error("create_dir_all", &directory, &error))?;
    let path = directory.join(JOURNAL_NAME);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(io_error("read", &path, &error)),
    };
    let (recovered, consumed) = editlog::decode_batches(&bytes)?;

    let mut file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| io_error("open", &path, &error))?;
    if consumed < bytes.len() {
        let length = u64::try_from(consumed).unwrap_or(u64::MAX);
        file.set_len(length)
            .map_err(|error| io_error("set_len", &path, &error))?;
        file.sync_all()
            .map_err(|error| io_error("sync_all", &path, &error))?;
    }
    file.seek(SeekFrom::End(0))
        .map_err(|error| io_error("seek", &path, &error))?;

    Ok(OpenedJournal {
        recovered,
        writer: JournalWriter::new(file, CommitPolicy::default()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::save::fixtures::{TempWorld, batch, hex};
    use std::io::Write;

    #[test]
    fn the_journal_round_trips_committed_batches() {
        let world = TempWorld::new("journal");
        let mut journal = open_journal(world.path()).expect("the journal opens");
        assert!(journal.recovered.is_empty());
        journal.writer.push(&batch(1)).expect("the batch encodes");
        journal.writer.flush().expect("the journal flushes");
        drop(journal);

        let reopened = open_journal(world.path()).expect("the journal reopens");
        assert_eq!(reopened.recovered.len(), 1);
        assert_eq!(reopened.recovered[0].revision, 1);
        assert!(reopened.recovered[0].edits[0].same_record(&batch(1).edits[0]));
    }

    #[test]
    fn a_corrupt_complete_frame_is_an_error_not_a_torn_tail() {
        let world = TempWorld::new("corrupt");
        let mut journal = open_journal(world.path()).expect("the journal opens");
        journal.writer.push(&batch(1)).expect("the batch encodes");
        journal.writer.flush().expect("the journal flushes");
        drop(journal);

        let path = world.path().join(JOURNAL_DIR).join(JOURNAL_NAME);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("the journal opens");
        let length = file.metadata().expect("the journal exists").len();
        file.seek(SeekFrom::Start(length - 1))
            .expect("the last byte is addressable");
        let mut byte = [0_u8; 1];
        std::io::Read::read_exact(&mut file, &mut byte).expect("the byte reads");
        file.seek(SeekFrom::Start(length - 1))
            .expect("the last byte is addressable");
        file.write_all(&[byte[0] ^ 0xFF]).expect("the byte flips");
        drop(file);

        assert!(open_journal(world.path()).is_err());
    }

    #[test]
    fn opening_truncates_a_torn_journal_tail() {
        let world = TempWorld::new("torn");
        let mut journal = open_journal(world.path()).expect("the journal opens");
        journal.writer.push(&batch(1)).expect("the batch encodes");
        journal.writer.flush().expect("the journal flushes");
        drop(journal);

        let path = world.path().join(JOURNAL_DIR).join(JOURNAL_NAME);
        let valid = fs::metadata(&path).expect("the journal exists").len();
        let mut file = OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("the journal appends");
        file.write_all(b"torn").expect("the tail writes");
        drop(file);

        let reopened = open_journal(world.path()).expect("the journal reopens");
        assert_eq!(reopened.recovered.len(), 1);
        assert_eq!(
            fs::metadata(&path).expect("the journal exists").len(),
            valid
        );
    }

    #[test]
    fn a_journal_frame_is_pinned() {
        let frame = crate::editlog::encode_batch(&batch(1));
        assert_eq!(
            hex(&frame),
            "3a0000000100000000000000000000000100000000000000010000000000000000000000000000000000000008000000636f72653a6169720a000000636f72653a73746f6e6552e8ab83",
            "pinned frame; changing it breaks every existing journal"
        );
    }
}
