//! Migration state, canonicalisation, and the report (`A16`).

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{ErrorKind, Stage, TerrainError};
use crate::header::{self, MigrationState};
use crate::save;

use super::io_error;
use super::world::WorldSave;

impl WorldSave {
    /// Starts a migration and records it in the header (`A16`).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::InvariantViolated`] for a backwards target, or
    /// [`ErrorKind::IoError`] when the header or journal cannot be written.
    pub fn begin_migration(
        &mut self,
        from_format: u16,
        to_format: u16,
    ) -> Result<(), TerrainError> {
        if from_format >= to_format {
            return Err(TerrainError::new(
                Stage::Persistence,
                ErrorKind::InvariantViolated {
                    check: "migration_target",
                    coord: None,
                },
            ));
        }
        self.header.migration_state = MigrationState::InProgress {
            from_format,
            to_format,
            started: header::now_unix_seconds(),
        };
        self.header_dirty = true;
        self.write_header()?;
        let path = self.root.join(save::MIGRATION_JOURNAL);
        fs::write(&path, b"").map_err(|error| io_error("write", &path, &error))?;
        save::sync_directory(&self.root)
    }

    /// Transforms every saved section to the canonical current record.
    ///
    /// Sections already recorded with the same post-transform CRC are skipped,
    /// so a resumed migration is idempotent. Each transformed file is recorded
    /// in `migration.journal` before the next one starts.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::InvariantViolated`] when no compatible migration is open,
    /// [`ErrorKind::CorruptSection`] for a bad record, and
    /// [`ErrorKind::IoError`] for a filesystem failure.
    pub fn migrate_sections(&mut self) -> Result<usize, TerrainError> {
        let MigrationState::InProgress { to_format, .. } = self.header.migration_state else {
            return Err(TerrainError::new(
                Stage::Persistence,
                ErrorKind::InvariantViolated {
                    check: "migration_state",
                    coord: None,
                },
            ));
        };
        if to_format != crate::section::SECTION_RECORD_VERSION {
            return Err(TerrainError::new(
                Stage::Persistence,
                ErrorKind::InvariantViolated {
                    check: "migration_section_format",
                    coord: None,
                },
            ));
        }
        let path = self.root.join(save::MIGRATION_JOURNAL);
        let (records, valid) = save::read_migration_records(&path)?;
        if let Ok(metadata) = fs::metadata(&path)
            && valid < metadata.len()
        {
            let file = OpenOptions::new()
                .write(true)
                .open(&path)
                .map_err(|error| io_error("open", &path, &error))?;
            file.set_len(valid)
                .map_err(|error| io_error("set_len", &path, &error))?;
            file.sync_all()
                .map_err(|error| io_error("sync_all", &path, &error))?;
        }

        let mut migrated = 0;
        for saved in self.read_sections()? {
            let section_path = self.sections.path_for(saved.key);
            let old_bytes =
                fs::read(&section_path).map_err(|error| io_error("read", &section_path, &error))?;
            let old_crc = header::crc32(&old_bytes);
            if records
                .iter()
                .any(|record| record.key == saved.key && record.new_crc == old_crc)
            {
                continue;
            }
            let new_bytes = self.sections.canonical_bytes(&saved);
            let new_crc = header::crc32(&new_bytes);
            if new_crc != old_crc {
                self.sections.replace(saved.key, &new_bytes)?;
            }
            save::append_migration_record(
                &path,
                save::MigrationRecord {
                    key: saved.key,
                    old_crc,
                    new_crc,
                },
            )?;
            migrated += 1;
        }
        Ok(migrated)
    }

    /// Re-runs [`Self::migrate_sections`]; completed files are skipped by CRC.
    ///
    /// # Errors
    ///
    /// The same errors as [`Self::migrate_sections`].
    pub fn resume_migration(&mut self) -> Result<usize, TerrainError> {
        self.migrate_sections()
    }

    /// Clears the migration state, writes the header, and deletes the journal.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::IoError`] for a filesystem failure.
    pub fn finish_migration(&mut self) -> Result<(), TerrainError> {
        self.header.migration_state = MigrationState::None;
        self.header.format_version = header::HEADER_FORMAT_VERSION;
        self.header_dirty = true;
        self.write_header()?;
        let path = self.root.join(save::MIGRATION_JOURNAL);
        if path.exists() {
            fs::remove_file(&path).map_err(|error| io_error("remove_file", &path, &error))?;
            save::sync_directory(&self.root)?;
        }
        Ok(())
    }

    /// Begins, runs, and finishes one migration.
    ///
    /// # Errors
    ///
    /// The same errors as the three steps.
    pub fn migrate(&mut self, from_format: u16, to_format: u16) -> Result<usize, TerrainError> {
        self.begin_migration(from_format, to_format)?;
        let migrated = self.migrate_sections()?;
        self.finish_migration()?;
        Ok(migrated)
    }

    /// Begins, runs, finishes, and reports one migration.
    ///
    /// # Errors
    ///
    /// The same errors as the three steps, or an [`ErrorKind::IoError`] while
    /// writing the report.
    pub fn migrate_with_report(
        &mut self,
        from_format: u16,
        to_format: u16,
    ) -> Result<MigrationReport, TerrainError> {
        let started = header::now_unix_seconds();
        let sections = self.migrate(from_format, to_format)?;
        let report = MigrationReport {
            from_format,
            to_format,
            started,
            finished: header::now_unix_seconds(),
            sections,
            ok: true,
            error: None,
        };
        write_migration_report(&self.root, &report)?;
        Ok(report)
    }
}

/// Prefix of the JSON report written after a migration (`A16`).
pub(crate) const MIGRATION_REPORT_PREFIX: &str = "migration-report-";

/// One migration report, written beside the header.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationReport {
    /// Format migrated from.
    pub from_format: u16,
    /// Format migrated to.
    pub to_format: u16,
    /// UTC Unix seconds when the migration started.
    pub started: i64,
    /// UTC Unix seconds when it finished (or failed).
    pub finished: i64,
    /// Sections processed (written or already covered).
    pub sections: usize,
    /// Whether every step completed.
    pub ok: bool,
    /// Failure text when `ok` is false.
    pub error: Option<String>,
}

/// Writes `migration-report-<finished>.json` and returns its path.
///
/// # Errors
///
/// [`ErrorKind::IoError`] for a filesystem failure.
pub fn write_migration_report(
    root: &Path,
    report: &MigrationReport,
) -> Result<PathBuf, TerrainError> {
    let name = format!("{MIGRATION_REPORT_PREFIX}{}.json", report.finished);
    let path = root.join(name);
    let temp = path.with_extension("json.tmp");
    let error = report.error.as_deref().map_or_else(
        || "null".to_owned(),
        |message| format!("\"{}\"", json_escape(message)),
    );
    let body = format!(
        "{{\"schema\":\"valescent/migration-report@1\",\"from_format\":{},\"to_format\":{},\"started\":{},\"finished\":{},\"sections\":{},\"ok\":{},\"error\":{}}}\n",
        report.from_format,
        report.to_format,
        report.started,
        report.finished,
        report.sections,
        report.ok,
        error
    );
    let mut file = File::create(&temp).map_err(|error| io_error("create", &temp, &error))?;
    file.write_all(body.as_bytes())
        .map_err(|error| io_error("write_all", &temp, &error))?;
    file.sync_all()
        .map_err(|error| io_error("sync_all", &temp, &error))?;
    fs::rename(&temp, &path).map_err(|error| io_error("rename", &temp, &error))?;
    save::sync_directory(root)?;
    Ok(path)
}

/// Escapes a report string for the tiny JSON writer above.
fn json_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content;
    use crate::world_save::fixtures::{TempWorld, key, uniform};

    #[test]
    fn a_migration_rewrites_sections_and_clears_the_state() {
        let world = TempWorld::new("migration");
        let mut opened =
            WorldSave::create(world.path(), 7, "Test world").expect("the world creates");
        let stone = content::v1_default()
            .id("core:stone")
            .expect("the v1 pack has stone");
        opened
            .save_section(key(0, 0, 0), [1; 32], true, &uniform(stone), 0)
            .expect("the section writes");
        let migrated = opened.migrate(1, 2).expect("the migration finishes");
        assert_eq!(migrated, 1);
        assert_eq!(opened.header.migration_state, MigrationState::None);
        assert!(!world.path().join(save::MIGRATION_JOURNAL).exists());
        drop(opened);

        let reopened = WorldSave::open(world.path(), 7).expect("the world reopens");
        assert_eq!(reopened.header.migration_state, MigrationState::None);
        assert_eq!(
            reopened.read_sections().expect("the sections read").len(),
            1
        );
    }

    #[test]
    fn a_resumed_migration_skips_recorded_sections() {
        let world = TempWorld::new("migration-resume");
        let mut opened =
            WorldSave::create(world.path(), 7, "Test world").expect("the world creates");
        let stone = content::v1_default()
            .id("core:stone")
            .expect("the v1 pack has stone");
        opened
            .save_section(key(0, 0, 0), [1; 32], true, &uniform(stone), 0)
            .expect("the section writes");
        opened.begin_migration(1, 2).expect("the migration begins");
        assert_eq!(opened.migrate_sections().expect("the first pass"), 1);
        assert_eq!(
            opened.resume_migration().expect("the resumed pass"),
            0,
            "the recorded section is skipped"
        );
        opened.finish_migration().expect("the migration finishes");
    }

    #[test]
    fn a_migration_writes_a_report() {
        let world = TempWorld::new("migration-report");
        let mut opened =
            WorldSave::create(world.path(), 7, "Test world").expect("the world creates");
        let stone = content::v1_default()
            .id("core:stone")
            .expect("the v1 pack has stone");
        opened
            .save_section(key(0, 0, 0), [1; 32], true, &uniform(stone), 0)
            .expect("the section writes");
        let report = opened
            .migrate_with_report(1, 2)
            .expect("the migration reports");
        assert_eq!(report.sections, 1);
        assert!(report.ok);
        let path = world
            .path()
            .join(format!("{MIGRATION_REPORT_PREFIX}{}.json", report.finished));
        let text = fs::read_to_string(&path).expect("the report reads");
        assert!(text.contains("\"ok\":true"));
        assert!(text.contains("\"sections\":1"));
    }
}
