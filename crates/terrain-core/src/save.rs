//! Save writer: section files, backup rotation, and the edit journal (`A16`).
//!
//! The on-disk format is normative in `docs/dev/save-format.md`; this module is
//! the code side of it. It names files, writes them atomically, rotates the one
//! previous generation, and opens the journal after recovering a torn tail.
//!
//! The journal IO thread is the caller's: [`open_journal`] returns the recovered
//! batches and a [`JournalWriter`] positioned at the recovered end. Call
//! [`JournalWriter::flush`] on pause, on quit, and before every section save.
//!
//! `world-repair` (the interactive choice between backup, quarantine, and
//! salvage) is not implemented here. [`SectionStore::read_backup`] exposes the
//! verified previous generation for a caller that offers it.

use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::content::ContentPack;
use crate::editlog::{self, CommitPolicy, EditBatch, JournalWriter};
use crate::error::{ErrorKind, SectionFault, Stage, TerrainError};
use crate::section::{self, SavedSection, Section, SectionKey};

/// Directory holding one file per section.
pub const SECTIONS_DIR: &str = "sections";
/// Directory holding append-only journal frames.
pub const JOURNAL_DIR: &str = "journal";
/// First journal segment. A new segment starts only when a save is interrupted.
pub const JOURNAL_NAME: &str = "0001.journal";
/// Append-only record of a migration in progress.
pub const MIGRATION_JOURNAL: &str = "migration.journal";
/// Bytes in one `migration.journal` record.
const MIGRATION_RECORD_LEN: usize = 22;

/// One completed section transform, for resume (`A16`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MigrationRecord {
    /// Section that was transformed.
    pub key: SectionKey,
    /// CRC of the section file before the transform.
    pub old_crc: u32,
    /// CRC of the section file after the transform.
    pub new_crc: u32,
}

impl MigrationRecord {
    /// Encodes the 22-byte record, checksum last.
    #[must_use]
    pub fn encode(self) -> [u8; MIGRATION_RECORD_LEN] {
        let mut out = [0_u8; MIGRATION_RECORD_LEN];
        out[0..4].copy_from_slice(&self.key.column[0].to_le_bytes());
        out[4..8].copy_from_slice(&self.key.column[1].to_le_bytes());
        out[8] = self.key.section;
        out[10..14].copy_from_slice(&self.old_crc.to_le_bytes());
        out[14..18].copy_from_slice(&self.new_crc.to_le_bytes());
        let checksum = crate::header::crc32(&out[..18]);
        out[18..22].copy_from_slice(&checksum.to_le_bytes());
        out
    }

    /// Decodes one record, returning `None` for a bad checksum or length.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let bytes: [u8; MIGRATION_RECORD_LEN] = bytes.try_into().ok()?;
        let checksum = crate::header::crc32(&bytes[..18]);
        let stored = u32::from_le_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]);
        if checksum != stored || bytes[9] != 0 {
            return None;
        }
        Some(Self {
            key: SectionKey {
                column: [
                    i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
                    i32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
                ],
                section: bytes[8],
            },
            old_crc: u32::from_le_bytes([bytes[10], bytes[11], bytes[12], bytes[13]]),
            new_crc: u32::from_le_bytes([bytes[14], bytes[15], bytes[16], bytes[17]]),
        })
    }
}

/// Reads complete migration records and reports the valid prefix.
///
/// A short tail is torn and returned unread; a complete record with a bad
/// checksum is an error, never silently dropped.
///
/// # Errors
///
/// [`ErrorKind::IoError`] for a filesystem failure and
/// [`ErrorKind::CorruptSection`] for a bad complete record.
pub fn read_migration_records(path: &Path) -> Result<(Vec<MigrationRecord>, u64), TerrainError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), 0)),
        Err(error) => return Err(io_error("read", path, &error)),
    };
    let mut records = Vec::new();
    let mut offset = 0;
    while offset + MIGRATION_RECORD_LEN <= bytes.len() {
        let record = MigrationRecord::decode(&bytes[offset..offset + MIGRATION_RECORD_LEN])
            .ok_or_else(|| corrupt(path, SectionFault::Checksum))?;
        records.push(record);
        offset += MIGRATION_RECORD_LEN;
    }
    Ok((records, u64::try_from(offset).unwrap_or(u64::MAX)))
}

/// Appends one migration record and syncs it.
///
/// # Errors
///
/// [`ErrorKind::IoError`] for a filesystem failure.
pub fn append_migration_record(path: &Path, record: MigrationRecord) -> Result<(), TerrainError> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| io_error("open", path, &error))?;
    file.write_all(&record.encode())
        .map_err(|error| io_error("write_all", path, &error))?;
    file.sync_data()
        .map_err(|error| io_error("sync_data", path, &error))
}

/// Current suffix (`S1.5`).
const CURRENT_SUFFIX: &str = ".sect";
/// In-flight suffix, never trusted directly (`S1.5`).
const TEMP_SUFFIX: &str = ".tmp";
/// Previous-generation suffix (`S1.5`).
const BACKUP_SUFFIX: &str = ".bak";

/// Writes and reads section files under one world directory.
#[derive(Clone, Debug)]
pub struct SectionStore {
    root: PathBuf,
    pack: ContentPack,
}

impl SectionStore {
    /// A store over `root`, without touching the filesystem.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>, pack: ContentPack) -> Self {
        Self {
            root: root.into(),
            pack,
        }
    }

    /// A store over `root`, creating `sections/` when it is missing.
    ///
    /// # Errors
    ///
    /// A filesystem error is [`ErrorKind::IoError`] with the failing operation.
    pub fn create(root: impl Into<PathBuf>, pack: ContentPack) -> Result<Self, TerrainError> {
        let store = Self::new(root, pack);
        fs::create_dir_all(store.sections_dir())
            .map_err(|error| io_error("create_dir_all", store.sections_dir(), &error))?;
        Ok(store)
    }

    /// The world directory this store writes under.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The directory holding section files.
    #[must_use]
    pub fn sections_dir(&self) -> PathBuf {
        self.root.join(SECTIONS_DIR)
    }

    /// The authoritative file for one section.
    #[must_use]
    pub fn path_for(&self, key: SectionKey) -> PathBuf {
        self.sections_dir().join(format!(
            "c.{}.{}.{}{}",
            key.column[0], key.column[1], key.section, CURRENT_SUFFIX
        ))
    }

    /// The previous-generation file for one section.
    #[must_use]
    pub fn backup_path_for(&self, key: SectionKey) -> PathBuf {
        with_suffix(&self.path_for(key), BACKUP_SUFFIX)
    }

    /// The in-flight file for one section.
    #[must_use]
    pub fn temp_path_for(&self, key: SectionKey) -> PathBuf {
        with_suffix(&self.path_for(key), TEMP_SUFFIX)
    }

    /// Writes one section, rotating the previous generation to `.bak`.
    ///
    /// Sequence (`S1.5`): write `.tmp`, fsync it, re-read and verify its CRC,
    /// rename current to `.bak` (replacing), rename `.tmp` to current, then
    /// fsync the directory where the platform supports it. The caller drops the
    /// journal prefix only after this returns `Ok`.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::IoError`] for a filesystem failure, and
    /// [`ErrorKind::CorruptSection`] if the freshly written file does not
    /// verify (which means the disk or the encoding lies, not the caller).
    pub fn write(
        &self,
        key: SectionKey,
        baseline_hash: [u8; 32],
        edited: bool,
        section: &Section,
    ) -> Result<(), TerrainError> {
        let current = self.path_for(key);
        let temp = self.temp_path_for(key);
        let backup = self.backup_path_for(key);
        let bytes = section::encode_record(&self.pack, key, baseline_hash, edited, section);

        {
            let mut file =
                File::create(&temp).map_err(|error| io_error("create", &temp, &error))?;
            file.write_all(&bytes)
                .map_err(|error| io_error("write_all", &temp, &error))?;
            file.sync_all()
                .map_err(|error| io_error("sync_all", &temp, &error))?;
        }

        let written = fs::read(&temp).map_err(|error| io_error("read", &temp, &error))?;
        let saved = section::decode_record_at(&self.pack, &temp, &written)?;
        if saved.key != key {
            return Err(corrupt(&temp, SectionFault::Framing));
        }

        if current.exists() {
            if backup.exists() {
                fs::remove_file(&backup)
                    .map_err(|error| io_error("remove_file", &backup, &error))?;
            }
            fs::rename(&current, &backup).map_err(|error| io_error("rename", &current, &error))?;
        }
        fs::rename(&temp, &current).map_err(|error| io_error("rename", &temp, &error))?;
        sync_directory(&self.sections_dir())?;
        Ok(())
    }

    /// Reads one section, recovering the `.tmp` rules on open (`S1.5`).
    ///
    /// If the current file is missing and `.tmp` verifies, `.tmp` is promoted.
    /// A stale `.tmp` beside a valid current file is deleted. A current file
    /// that fails its CRC is an error and is never silently replaced; use
    /// [`Self::read_backup`] to offer the previous generation.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::IoError`] for a filesystem failure and
    /// [`ErrorKind::CorruptSection`] for a damaged current or `.tmp` file.
    pub fn read(&self, key: SectionKey) -> Result<Option<SavedSection>, TerrainError> {
        let current = self.path_for(key);
        let temp = self.temp_path_for(key);
        if current.is_file() {
            if temp.is_file() {
                let _ = fs::remove_file(&temp);
            }
            let bytes = fs::read(&current).map_err(|error| io_error("read", &current, &error))?;
            let saved = section::decode_record_at(&self.pack, &current, &bytes)?;
            if saved.key != key {
                return Err(corrupt(&current, SectionFault::Framing));
            }
            return Ok(Some(saved));
        }
        if temp.is_file() {
            let bytes = fs::read(&temp).map_err(|error| io_error("read", &temp, &error))?;
            let saved = section::decode_record_at(&self.pack, &temp, &bytes)?;
            if saved.key != key {
                return Err(corrupt(&temp, SectionFault::Framing));
            }
            fs::rename(&temp, &current).map_err(|error| io_error("rename", &temp, &error))?;
            sync_directory(&self.sections_dir())?;
            return Ok(Some(saved));
        }
        Ok(None)
    }

    /// Reads every saved section under `sections/`, in key order.
    ///
    /// `.tmp` and `.bak` files are ignored; a `.sect` name that does not parse
    /// as a section key is skipped rather than guessed at.
    ///
    /// # Errors
    ///
    /// The same errors as [`Self::read`], for any current or promoted file.
    pub fn read_all(&self) -> Result<Vec<SavedSection>, TerrainError> {
        let directory = self.sections_dir();
        if !directory.is_dir() {
            return Ok(Vec::new());
        }
        let mut keys = Vec::new();
        for entry in
            fs::read_dir(&directory).map_err(|error| io_error("read_dir", &directory, &error))?
        {
            let entry = entry.map_err(|error| io_error("read_dir", &directory, &error))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if let Some(key) = parse_section_name(name) {
                keys.push(key);
            }
        }
        keys.sort_unstable();
        let mut sections = Vec::with_capacity(keys.len());
        for key in keys {
            if let Some(saved) = self.read(key)? {
                sections.push(saved);
            }
        }
        Ok(sections)
    }

    /// The section key named by a file path, when it is a section file.
    #[must_use]
    pub fn key_from_path(&self, path: &Path) -> Option<SectionKey> {
        path.file_name()
            .and_then(std::ffi::OsStr::to_str)
            .and_then(parse_section_name)
    }

    /// The canonical record bytes for a decoded section.
    #[must_use]
    pub fn canonical_bytes(&self, saved: &SavedSection) -> Vec<u8> {
        section::encode_record(
            &self.pack,
            saved.key,
            saved.baseline_hash,
            saved.edited,
            &saved.section,
        )
    }

    /// Replaces a section's file with verified record bytes, without rotation.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::IoError`] for a filesystem failure and
    /// [`ErrorKind::CorruptSection`] when the bytes do not verify.
    pub fn replace(&self, key: SectionKey, bytes: &[u8]) -> Result<(), TerrainError> {
        self.install_bytes(key, bytes)
    }

    /// Reads and verifies the previous generation, if any.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::IoError`] for a filesystem failure and
    /// [`ErrorKind::CorruptSection`] for a damaged backup.
    pub fn read_backup(&self, key: SectionKey) -> Result<Option<SavedSection>, TerrainError> {
        let backup = self.backup_path_for(key);
        if !backup.is_file() {
            return Ok(None);
        }
        let bytes = fs::read(&backup).map_err(|error| io_error("read", &backup, &error))?;
        let saved = section::decode_record_at(&self.pack, &backup, &bytes)?;
        if saved.key != key {
            return Err(corrupt(&backup, SectionFault::Framing));
        }
        Ok(Some(saved))
    }

    /// The quarantined file for one damaged section (`S1.5`).
    #[must_use]
    pub fn corrupt_path_for(&self, key: SectionKey) -> PathBuf {
        with_suffix(&self.path_for(key), ".corrupt")
    }

    /// Offers the three `world-repair` outcomes for a damaged current section.
    ///
    /// [`RepairAction::RestoreBackup`] verifies `.bak`, moves the current file
    /// to `.corrupt`, and installs the backup bytes as the new current without
    /// consuming `.bak`. [`RepairAction::Quarantine`] moves the current file to
    /// `.corrupt` and leaves no current. [`RepairAction::Salvage`] returns the
    /// verified backup in memory and touches nothing.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::IoError`] for a filesystem failure or a missing backup when
    /// one is needed, and [`ErrorKind::CorruptSection`] for a damaged backup.
    pub fn repair(
        &self,
        key: SectionKey,
        action: RepairAction,
    ) -> Result<RepairOutcome, TerrainError> {
        match action {
            RepairAction::RestoreBackup => {
                let backup = self.backup_path_for(key);
                let bytes = fs::read(&backup).map_err(|error| io_error("read", &backup, &error))?;
                let saved = section::decode_record_at(&self.pack, &backup, &bytes)?;
                if saved.key != key {
                    return Err(corrupt(&backup, SectionFault::Framing));
                }
                self.quarantine_current(key)?;
                self.install_bytes(key, &bytes)?;
                Ok(RepairOutcome::Restored(saved))
            }
            RepairAction::Quarantine => {
                self.quarantine_current(key)?;
                Ok(RepairOutcome::Quarantined)
            }
            RepairAction::Salvage => {
                let Some(saved) = self.read_backup(key)? else {
                    return Err(missing_backup(&self.backup_path_for(key)));
                };
                Ok(RepairOutcome::Salvaged(saved))
            }
        }
    }

    /// Moves the current file aside as `.corrupt`.
    fn quarantine_current(&self, key: SectionKey) -> Result<(), TerrainError> {
        let current = self.path_for(key);
        if !current.is_file() {
            return Err(missing_backup(&current));
        }
        let target = self.corrupt_path_for(key);
        if target.exists() {
            fs::remove_file(&target).map_err(|error| io_error("remove_file", &target, &error))?;
        }
        fs::rename(&current, &target).map_err(|error| io_error("rename", &current, &error))?;
        let temp = self.temp_path_for(key);
        if temp.exists() {
            let _ = fs::remove_file(&temp);
        }
        sync_directory(&self.sections_dir())
    }

    /// Writes verified record bytes as the current section, without rotation.
    fn install_bytes(&self, key: SectionKey, bytes: &[u8]) -> Result<(), TerrainError> {
        let temp = self.temp_path_for(key);
        let current = self.path_for(key);
        {
            let mut file =
                File::create(&temp).map_err(|error| io_error("create", &temp, &error))?;
            file.write_all(bytes)
                .map_err(|error| io_error("write_all", &temp, &error))?;
            file.sync_all()
                .map_err(|error| io_error("sync_all", &temp, &error))?;
        }
        let written = fs::read(&temp).map_err(|error| io_error("read", &temp, &error))?;
        let saved = section::decode_record_at(&self.pack, &temp, &written)?;
        if saved.key != key {
            return Err(corrupt(&temp, SectionFault::Framing));
        }
        fs::rename(&temp, &current).map_err(|error| io_error("rename", &temp, &error))?;
        sync_directory(&self.sections_dir())
    }
}

/// What `world-repair` should do with a damaged current section (`S1.5`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepairAction {
    /// Replace the current section with the verified `.bak` generation.
    RestoreBackup,
    /// Move the current section aside as `.corrupt` and leave no current.
    Quarantine,
    /// Return the verified `.bak` generation without changing the disk.
    Salvage,
}

/// The result of [`SectionStore::repair`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RepairOutcome {
    /// The backup was verified and installed as the current section.
    Restored(SavedSection),
    /// The damaged current file was moved to `.corrupt`.
    Quarantined,
    /// The backup was verified and returned; the disk is unchanged.
    Salvaged(SavedSection),
}

/// The recovered journal, positioned after its last complete frame.
pub struct OpenedJournal {
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
/// The valid prefix is decoded with [`editlog::decode_batches`]; when the file
/// ends mid-frame the file is truncated to the recovered offset before the
/// writer is constructed, which is the [`JournalWriter`] contract.
///
/// # Errors
///
/// [`ErrorKind::IoError`] for a filesystem failure and the framing errors from
/// [`editlog::decode_batches`] for a corrupt complete frame.
pub fn open_journal(root: impl AsRef<Path>) -> Result<OpenedJournal, TerrainError> {
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

/// Parses `c.<cx>.<cz>.<sy>.sect`, the name [`SectionStore::path_for`] writes.
fn parse_section_name(name: &str) -> Option<SectionKey> {
    let rest = name.strip_prefix("c.")?.strip_suffix(CURRENT_SUFFIX)?;
    let mut parts = rest.split('.');
    let cx = parts.next()?.parse().ok()?;
    let cz = parts.next()?.parse().ok()?;
    let section = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(SectionKey {
        column: [cx, cz],
        section,
    })
}

/// Appends `suffix` to a path's file name.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// Fsyncs a directory where the platform exposes it.
///
/// Windows has no directory fsync through `std`; the renames are still ordered,
/// but crash durability there depends on the filesystem's metadata journal.
#[cfg(unix)]
pub(crate) fn sync_directory(path: &Path) -> Result<(), TerrainError> {
    let directory = File::open(path).map_err(|error| io_error("open", path, &error))?;
    directory
        .sync_all()
        .map_err(|error| io_error("sync_all", path, &error))?;
    Ok(())
}

/// Windows has no directory fsync; the renames are still ordered.
#[cfg(not(unix))]
#[allow(
    clippy::unnecessary_wraps,
    reason = "the call site matches the unix signature that can fail"
)]
pub(crate) fn sync_directory(_path: &Path) -> Result<(), TerrainError> {
    Ok(())
}

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

/// The error for a repair that needs a section or backup file that is absent.
fn missing_backup(path: &Path) -> TerrainError {
    TerrainError::new(
        Stage::Persistence,
        ErrorKind::IoError {
            op: "read_backup",
            path: path.to_path_buf(),
            os_error: 0,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content;
    use crate::description::SECTION_VOLUME;
    use crate::editlog::{Edit, EditBatch};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    /// A unique world directory, removed when the test ends.
    struct TempWorld(PathBuf);

    impl TempWorld {
        fn new(tag: &str) -> Self {
            let serial = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
            let directory = std::env::temp_dir().join(format!(
                "valescent-save-{}-{tag}-{serial}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&directory);
            Self(directory)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempWorld {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn pack() -> ContentPack {
        content::v1_default()
    }

    fn id(name: &str) -> crate::section::BlockId {
        pack().id(name).expect("the v1 pack has the name")
    }

    fn uniform(block: crate::section::BlockId) -> Section {
        let blocks: Box<[crate::section::BlockId; SECTION_VOLUME]> = vec![block; SECTION_VOLUME]
            .into_boxed_slice()
            .try_into()
            .expect("the section volume matches");
        Section::from_blocks(&blocks)
    }

    fn key() -> SectionKey {
        SectionKey {
            column: [2, -3],
            section: 4,
        }
    }

    fn baseline() -> [u8; 32] {
        [7; 32]
    }

    #[test]
    fn write_then_read_round_trips_a_section() {
        let world = TempWorld::new("round-trip");
        let store = SectionStore::create(world.path(), pack()).expect("the store opens");
        let section = uniform(id("core:stone"));

        store
            .write(key(), baseline(), true, &section)
            .expect("the section writes");
        let saved = store
            .read(key())
            .expect("the section reads")
            .expect("the section exists");
        assert_eq!(saved.key, key());
        assert_eq!(saved.baseline_hash, baseline());
        assert!(saved.edited);
        assert_eq!(saved.section, section);
        assert!(!store.temp_path_for(key()).exists());
    }

    #[test]
    fn a_second_write_rotates_exactly_one_backup() {
        let world = TempWorld::new("backup");
        let store = SectionStore::create(world.path(), pack()).expect("the store opens");
        let first = uniform(id("core:stone"));
        let second = uniform(id("core:dirt"));

        store
            .write(key(), baseline(), true, &first)
            .expect("the first generation writes");
        store
            .write(key(), baseline(), true, &second)
            .expect("the second generation writes");

        let current = store
            .read(key())
            .expect("the current reads")
            .expect("the current exists");
        assert_eq!(current.section, second);
        let backup = store
            .read_backup(key())
            .expect("the backup reads")
            .expect("the backup exists");
        assert_eq!(backup.section, first);
    }

    #[test]
    fn writing_without_a_current_file_keeps_the_existing_backup() {
        let world = TempWorld::new("missing-current");
        let store = SectionStore::create(world.path(), pack()).expect("the store opens");
        let first = uniform(id("core:stone"));
        let second = uniform(id("core:dirt"));
        store
            .write(key(), baseline(), true, &first)
            .expect("the first generation writes");
        fs::rename(store.path_for(key()), store.backup_path_for(key()))
            .expect("the current becomes the backup");

        store
            .write(key(), baseline(), true, &second)
            .expect("the second generation writes");
        assert_eq!(
            store
                .read(key())
                .expect("the current reads")
                .expect("the current exists")
                .section,
            second
        );
        assert_eq!(
            store
                .read_backup(key())
                .expect("the backup reads")
                .expect("the backup survives")
                .section,
            first
        );
    }

    #[test]
    fn a_missing_current_promotes_a_verified_temp_file() {
        let world = TempWorld::new("promote");
        let store = SectionStore::create(world.path(), pack()).expect("the store opens");
        let section = uniform(id("core:stone"));
        let bytes = section::encode_record(&pack(), key(), baseline(), true, &section);
        fs::write(store.temp_path_for(key()), bytes).expect("the temp file writes");

        let saved = store
            .read(key())
            .expect("the temp promotes")
            .expect("the section exists");
        assert_eq!(saved.section, section);
        assert!(store.path_for(key()).is_file());
        assert!(!store.temp_path_for(key()).exists());
    }

    #[test]
    fn a_corrupt_temp_file_is_refused_and_not_promoted() {
        let world = TempWorld::new("bad-temp");
        let store = SectionStore::create(world.path(), pack()).expect("the store opens");
        fs::write(store.temp_path_for(key()), b"TSEC-not-a-record").expect("the temp writes");

        let error = store.read(key()).expect_err("a bad temp is refused");
        assert!(matches!(
            error.kind,
            ErrorKind::CorruptSection {
                fault: SectionFault::Length | SectionFault::Framing | SectionFault::Checksum,
                ..
            }
        ));
        assert!(!store.path_for(key()).exists());
    }

    #[test]
    fn a_stale_temp_file_is_removed_when_the_current_verifies() {
        let world = TempWorld::new("stale-temp");
        let store = SectionStore::create(world.path(), pack()).expect("the store opens");
        store
            .write(key(), baseline(), true, &uniform(id("core:stone")))
            .expect("the section writes");
        fs::write(store.temp_path_for(key()), b"stale").expect("the stale temp writes");

        let saved = store
            .read(key())
            .expect("the current reads")
            .expect("the section exists");
        assert_eq!(saved.section, uniform(id("core:stone")));
        assert!(!store.temp_path_for(key()).exists());
    }

    #[test]
    fn restore_backup_installs_the_previous_generation() {
        let world = TempWorld::new("repair-restore");
        let store = SectionStore::create(world.path(), pack()).expect("the store opens");
        let first = uniform(id("core:stone"));
        let second = uniform(id("core:dirt"));
        store
            .write(key(), baseline(), true, &first)
            .expect("the first generation writes");
        store
            .write(key(), baseline(), true, &second)
            .expect("the second generation writes");
        fs::write(store.path_for(key()), b"broken current").expect("the current is damaged");

        let outcome = store
            .repair(key(), RepairAction::RestoreBackup)
            .expect("the backup restores");
        assert!(matches!(outcome, RepairOutcome::Restored(ref saved) if saved.section == first));
        assert_eq!(
            store.read(key()).unwrap().unwrap().section,
            first,
            "the current is the previous generation"
        );
        assert_eq!(
            store.read_backup(key()).unwrap().unwrap().section,
            first,
            "the backup is kept"
        );
        assert!(store.corrupt_path_for(key()).is_file());
    }

    #[test]
    fn quarantine_moves_the_damaged_current_aside() {
        let world = TempWorld::new("repair-quarantine");
        let store = SectionStore::create(world.path(), pack()).expect("the store opens");
        store
            .write(key(), baseline(), true, &uniform(id("core:stone")))
            .expect("the section writes");
        fs::write(store.path_for(key()), b"broken current").expect("the current is damaged");

        assert_eq!(
            store
                .repair(key(), RepairAction::Quarantine)
                .expect("the current quarantines"),
            RepairOutcome::Quarantined
        );
        assert!(!store.path_for(key()).exists());
        assert!(store.corrupt_path_for(key()).is_file());
    }

    #[test]
    fn salvage_returns_the_backup_without_touching_the_disk() {
        let world = TempWorld::new("repair-salvage");
        let store = SectionStore::create(world.path(), pack()).expect("the store opens");
        let first = uniform(id("core:stone"));
        store
            .write(key(), baseline(), true, &first)
            .expect("the first generation writes");
        store
            .write(key(), baseline(), true, &uniform(id("core:dirt")))
            .expect("the second generation writes");
        fs::write(store.path_for(key()), b"broken current").expect("the current is damaged");
        let damaged = fs::read(store.path_for(key())).expect("the damaged bytes read");

        let outcome = store
            .repair(key(), RepairAction::Salvage)
            .expect("the backup salvages");
        assert_eq!(
            outcome,
            RepairOutcome::Salvaged(store.read_backup(key()).unwrap().unwrap())
        );
        assert_eq!(
            fs::read(store.path_for(key())).expect("the current reads"),
            damaged,
            "salvage leaves the current alone"
        );
        assert!(!store.corrupt_path_for(key()).exists());
    }

    #[test]
    fn migration_records_round_trip_and_ignore_a_torn_tail() {
        let world = TempWorld::new("migration-record");
        fs::create_dir_all(world.path()).expect("the world directory creates");
        let path = world.path().join(MIGRATION_JOURNAL);
        let record = MigrationRecord {
            key: key(),
            old_crc: 0x1234,
            new_crc: 0x5678,
        };
        append_migration_record(&path, record).expect("the record appends");
        let mut file = OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("the journal appends");
        file.write_all(b"torn").expect("the tail writes");
        drop(file);

        let (records, valid) = read_migration_records(&path).expect("the records read");
        assert_eq!(records, vec![record]);
        assert_eq!(valid, 22);

        let mut bad = record.encode();
        bad[10] ^= 0xFF;
        assert!(MigrationRecord::decode(&bad).is_none());
    }

    #[test]
    fn section_record_corruption_never_panics() {
        let pack = pack();
        let bytes =
            section::encode_record(&pack, key(), baseline(), true, &uniform(id("core:stone")));
        for index in 0..bytes.len() {
            let mut damaged = bytes.clone();
            damaged[index] ^= 0xFF;
            let _ = section::decode_record(&pack, &damaged);
        }
        let mut bad = bytes;
        let last = bad.len() - 1;
        bad[last] ^= 0xFF;
        assert!(section::decode_record(&pack, &bad).is_err());
    }

    fn batch(revision: u64) -> EditBatch {
        let record = Edit::record([0, 1, 0], "core:air", "core:stone", revision, 0, 0, 0)
            .expect("the change is recorded");
        EditBatch::new(revision, 0, vec![record])
    }

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
}
