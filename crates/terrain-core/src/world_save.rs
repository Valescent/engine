//! World-level save orchestration: header, description, sections, journal.
//!
//! [`WorldSave`] enforces the load order from `A1.5` in one place: header CRC
//! and format, description present and parsed, recomputed description hash,
//! pinned generator version, then journal recovery. It composes the pieces in
//! [`crate::save`] and [`crate::editlog`]; it does not generate terrain and does
//! not decide which sections are dirty.
//!
//! The engine still owns the IO loop. Call [`WorldSave::record_batch`] for each
//! committed edit, [`WorldSave::flush`] on pause and quit, and
//! [`WorldSave::save_section`] after a section's edits are durable in memory.
//!
//! # Structure
//!
//! `world` holds [`WorldSave`], the open/create order, the header revision, and
//! section reads; `worker` the journal IO thread; `migrate` the migration state,
//! canonicalisation, and the report. Every item is re-exported here, so
//! `world_save::<item>` paths are unchanged.

#[cfg(test)]
mod fixtures;
mod migrate;
mod worker;
mod world;

pub use migrate::{MIGRATION_REPORT_PREFIX, MigrationReport, write_migration_report};
pub use worker::SaveWorker;
pub use world::{DESCRIPTION_FILE, GENERATOR_VERSION, WorldSave};

use std::path::Path;

use crate::error::{ErrorKind, Stage, TerrainError};

/// Wraps an OS error as a persistence error naming the file and operation.
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
