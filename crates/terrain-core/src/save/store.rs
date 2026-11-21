//! Section files: naming, atomic writes, backup rotation, and repair.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::content::ContentPack;
use crate::error::{ErrorKind, SectionFault, Stage, TerrainError};
use crate::section::{self, SavedSection, Section, SectionKey};

use super::{corrupt, io_error};

/// Directory holding one file per section.
pub(crate) const SECTIONS_DIR: &str = "sections";

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::save::fixtures::{TempWorld, baseline, hex, id, key, pack, uniform};

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

    #[test]
    fn an_encoded_section_record_is_pinned() {
        let bytes =
            section::encode_record(&pack(), key(), baseline(), true, &uniform(id("core:stone")));
        assert_eq!(
            hex(&bytes),
            "54534543020002000000fdffffff0401070707070707070707070707070707070707070707070707070707070707070700008000000a00636f72653a73746f6e6596a44e1a",
            "pinned record; changing it changes every saved section file"
        );
    }
}
