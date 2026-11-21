//! Save writer: section files, backup rotation, and the edit journal (`A16`).
//!
//! The on-disk format is normative in `docs/dev/save-format.md`; this module is
//! the code side of it. It names files, writes them atomically, rotates the one
//! previous generation, and opens the journal after recovering a torn tail.
//!
//! The journal IO thread is the caller's: `open_journal` returns the recovered
//! batches and a [`JournalWriter`](crate::editlog::JournalWriter) positioned at
//! the recovered end. Call
//! [`JournalWriter::flush`](crate::editlog::JournalWriter::flush) on pause, on quit, and before every section save.
//!
//! `world-repair` (the interactive choice between backup, quarantine, and
//! salvage) is not implemented here. [`SectionStore::read_backup`] exposes the
//! verified previous generation for a caller that offers it.
//!
//! # Structure
//!
//! `store` holds [`SectionStore`] and the repair actions: the file naming, the
//! temp/verify/rotate/rename sequence, and the directory fsync. `journal` opens
//! and recovers the append-only journal, and `record` codes the migration
//! journal records. Every item is re-exported here, so `save::<item>` paths are
//! unchanged.

#[cfg(test)]
mod fixtures;
mod journal;
mod record;
mod store;

#[allow(unused_imports)] // crate-private re-exports keep the flat module paths
pub(crate) use journal::{JOURNAL_DIR, JOURNAL_NAME, OpenedJournal, open_journal};
#[allow(unused_imports)] // crate-private re-exports keep the flat module paths
pub(crate) use record::{
    MIGRATION_JOURNAL, MigrationRecord, append_migration_record, read_migration_records,
};
#[allow(unused_imports)] // crate-private re-exports keep the flat module paths
pub(crate) use store::SECTIONS_DIR;
pub(crate) use store::sync_directory;
pub use store::{RepairAction, RepairOutcome, SectionStore};

use std::path::PathBuf;

use crate::error::{ErrorKind, SectionFault, Stage, TerrainError};

/// Wraps an OS error as a persistence error naming the file and operation.
fn io_error(op: &'static str, path: impl Into<PathBuf>, error: &std::io::Error) -> TerrainError {
    TerrainError::new(
        Stage::Persistence,
        ErrorKind::IoError {
            op,
            path: path.into(),
            os_error: error.raw_os_error().unwrap_or(0),
        },
    )
}

/// Builds a corrupt-section error for a file that failed validation.
fn corrupt(path: impl Into<PathBuf>, fault: SectionFault) -> TerrainError {
    TerrainError::new(
        Stage::Persistence,
        ErrorKind::CorruptSection {
            path: path.into(),
            fault,
        },
    )
}
