//! The open world: identity checks, the header, and section access.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::content;
use crate::description::{self, Description};
use crate::editlog::{self, EditBatch, JournalWriter};
use crate::error::{ErrorKind, Stage, TerrainError};
use crate::hash;
use crate::header::{self, HEADER_FILE, Header, MigrationState};
use crate::save::{self, SectionStore};
use crate::section::{SavedSection, Section, SectionKey};

use super::io_error;

/// Canonical generation parameters, beside the header.
pub const DESCRIPTION_FILE: &str = "description.toml";
/// Generator version this build writes and reads (`T1.2`).
///
/// Version 2 solved hydrology over the whole finite world instead of per tile,
/// so lake and river levels at a 512-block seam can differ from version 1.
/// Version 3 normalises the biome dither margin by its block-space gradient
/// (so a transition is a fixed number of blocks) and applies the per-column
/// `biome_pick` to the surface rules and the feature footprint. Version 4 moves
/// the desert centre to `(0.84, 0.15)` with weight `1.15`, which changes the
/// biome splits and therefore topsoil and feature placement.
pub const GENERATOR_VERSION: u16 = 4;
/// One-past-the-active counter for unique world ids within this process.
static NEXT_WORLD: AtomicU64 = AtomicU64::new(0);

/// One open world directory.
#[derive(Debug)]
pub struct WorldSave {
    pub(super) root: PathBuf,
    /// The verified header. Updated in place before it is written back.
    pub header: Header,
    /// The description the header's hash was verified against.
    pub description: Description,
    /// Section file access under `sections/`.
    pub sections: SectionStore,
    /// Append-only edit frames. `None` only while a trim replaces the file.
    journal: Option<JournalWriter<File>>,
    /// Batches recovered from the journal at open, in file order.
    recovered: Vec<EditBatch>,
    /// Whether `header.revision` changed and the header still needs writing.
    pub(super) header_dirty: bool,
}

impl WorldSave {
    /// Opens the world at `root`, creating it when `header.bin` is missing.
    ///
    /// # Errors
    ///
    /// The same errors as [`Self::open`] and [`Self::create`].
    pub fn open_or_create(
        root: impl Into<PathBuf>,
        seed: u64,
        name: &str,
    ) -> Result<Self, TerrainError> {
        let root = root.into();
        if root.join(HEADER_FILE).is_file() {
            Self::open(root, seed)
        } else {
            Self::create(root, seed, name)
        }
    }

    /// Creates a new world: description first, then the header commit point.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::IoError`] for a filesystem failure, or the description and
    /// header validation errors from their encoders.
    pub fn create(root: impl Into<PathBuf>, seed: u64, name: &str) -> Result<Self, TerrainError> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(|error| io_error("create_dir_all", &root, &error))?;
        let description = description::default_v1();
        write_description(&root, &description)?;
        let description_hash = description.description_hash()?;
        let now = header::now_unix_seconds();
        let platform = platform_tag();
        let header = Header {
            format_version: header::HEADER_FORMAT_VERSION,
            generator_version: GENERATOR_VERSION,
            description_hash,
            hash_algorithm: header::HASH_ALGORITHM.to_owned(),
            seed,
            platform_tag: platform.clone(),
            last_saved_platform_tag: platform,
            world_id: new_world_id(),
            name: name.to_owned(),
            created: now,
            last_saved: now,
            revision: 0,
            schema_version: description::SCHEMA_VERSION,
            migration_state: MigrationState::None,
        };
        header.write(&root.join(HEADER_FILE))?;
        Self::with_parts(root, header, description)
    }

    /// Opens an existing world and verifies its identity.
    ///
    /// # Errors
    ///
    /// `CorruptSection` for a bad header, `DescriptionHashMismatch` when the
    /// description changed since the world was created, `UnknownGeneratorVersion`
    /// for a version this build does not read, or `SeedMismatch` when the
    /// caller's seed differs from the header's.
    pub fn open(root: impl Into<PathBuf>, seed: u64) -> Result<Self, TerrainError> {
        let root = root.into();
        let mut header = Header::read(&root.join(HEADER_FILE))?;
        let description_path = root.join(DESCRIPTION_FILE);
        let text = fs::read_to_string(&description_path)
            .map_err(|error| io_error("read", &description_path, &error))?;
        let description = description::from_toml_str(&text)?;
        let recomputed = description.description_hash()?;
        if recomputed != header.description_hash {
            return Err(TerrainError::new(
                Stage::Persistence,
                ErrorKind::DescriptionHashMismatch {
                    expected: recomputed,
                    found: header.description_hash,
                },
            ));
        }
        if header.generator_version != GENERATOR_VERSION {
            return Err(TerrainError::new(
                Stage::Persistence,
                ErrorKind::UnknownGeneratorVersion(header.generator_version),
            ));
        }
        if header.seed != seed {
            return Err(TerrainError::new(
                Stage::Persistence,
                ErrorKind::SeedMismatch {
                    expected: seed,
                    found: header.seed,
                },
            ));
        }
        if header.format_version < header::HEADER_FORMAT_VERSION {
            header.format_version = header::HEADER_FORMAT_VERSION;
            header.write(&root.join(HEADER_FILE))?;
        }
        Self::with_parts(root, header, description)
    }

    /// Composes the stores and recovers the journal.
    fn with_parts(
        root: PathBuf,
        header: Header,
        description: Description,
    ) -> Result<Self, TerrainError> {
        let sections = SectionStore::create(root.clone(), content::v1_default())?;
        let opened = save::open_journal(&root)?;
        Ok(Self {
            root,
            header,
            description,
            sections,
            journal: Some(opened.writer),
            recovered: opened.recovered,
            header_dirty: false,
        })
    }

    /// The world directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Batches recovered from the journal when the world opened.
    #[must_use]
    pub fn recovered(&self) -> &[EditBatch] {
        &self.recovered
    }

    /// The journal writer, for a caller that owns the IO loop.
    pub fn journal_mut(&mut self) -> &mut JournalWriter<File> {
        self.journal.as_mut().expect("the journal is open")
    }

    /// Reads every saved section, in key order.
    ///
    /// # Errors
    ///
    /// The same errors as [`SectionStore::read_all`].
    pub fn read_sections(&self) -> Result<Vec<SavedSection>, TerrainError> {
        self.sections.read_all()
    }

    /// Buffers one committed edit batch.
    ///
    /// The batch is not durable until [`Self::flush`] or a policy-driven
    /// [`JournalWriter::flush_if_due`] on [`Self::journal_mut`]. The header
    /// revision advances only when a section covering the batch is saved, so a
    /// frame is never marked covered before its blocks are on disk.
    ///
    /// # Errors
    ///
    /// The encoding errors from [`JournalWriter::push`].
    pub fn record_batch(&mut self, batch: &EditBatch) -> Result<(), TerrainError> {
        let Some(journal) = self.journal.as_mut() else {
            return Err(closed_journal());
        };
        journal.push(batch)
    }

    /// Advances the header revision after replayed edits are saved.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::IoError`] when the header cannot be written.
    pub fn advance_revision(&mut self, revision: u64) -> Result<(), TerrainError> {
        if revision > self.header.revision {
            self.header.revision = revision;
            self.header_dirty = true;
        }
        self.write_header()
    }

    /// Makes the journal durable.
    ///
    /// The header is not rewritten: its revision means "the last revision whose
    /// section save completed", and a flush does not save a section.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::IoError`] from the journal.
    pub fn flush(&mut self) -> Result<(), TerrainError> {
        if let Some(journal) = self.journal.as_mut() {
            journal.flush()?;
        }
        Ok(())
    }

    /// Flushes the journal, writes one section, advances the header to
    /// `through`, then trims the covered journal prefix.
    ///
    /// The journal flush comes first, per the writer contract: a section file
    /// must never be ahead of the edits that produced it. `through` is the
    /// greatest revision the section's blocks include.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::IoError`] for a filesystem failure, or the encoding errors
    /// from [`SectionStore::write`].
    pub fn save_section(
        &mut self,
        key: SectionKey,
        baseline_hash: [u8; 32],
        edited: bool,
        section: &Section,
        through: u64,
    ) -> Result<(), TerrainError> {
        self.flush()?;
        self.sections.write(key, baseline_hash, edited, section)?;
        if through > self.header.revision {
            self.header.revision = through;
        }
        self.header.last_saved = header::now_unix_seconds();
        self.header.last_saved_platform_tag = platform_tag();
        self.header_dirty = true;
        self.write_header()?;
        self.trim_journal(through)
    }

    /// Drops journal frames at or below `through`.
    ///
    /// The caller has already saved every section those frames touch, so the
    /// frames remain only as an audit until this point. The file is replaced
    /// through a temp and a rename; the writer is reopened afterwards.
    fn trim_journal(&mut self, through: u64) -> Result<(), TerrainError> {
        if through > self.header.revision {
            return Ok(());
        }
        let path = self.root.join(save::JOURNAL_DIR).join(save::JOURNAL_NAME);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(io_error("read", &path, &error)),
        };
        let (batches, _) = editlog::decode_batches(&bytes)?;
        if !batches.iter().any(|batch| batch.revision <= through) {
            return Ok(());
        }
        let mut kept = Vec::new();
        for batch in batches.iter().filter(|batch| batch.revision > through) {
            kept.extend_from_slice(&editlog::encode_batch(batch));
        }

        self.flush()?;
        let Some(writer) = self.journal.take() else {
            return Ok(());
        };
        drop(writer.into_sink());

        let temp = path.with_extension("journal.tmp");
        let written = (|| {
            let mut file =
                File::create(&temp).map_err(|error| io_error("create", &temp, &error))?;
            file.write_all(&kept)
                .map_err(|error| io_error("write_all", &temp, &error))?;
            file.sync_all()
                .map_err(|error| io_error("sync_all", &temp, &error))?;
            fs::rename(&temp, &path).map_err(|error| io_error("rename", &temp, &error))?;
            if let Some(parent) = path.parent() {
                save::sync_directory(parent)?;
            }
            Ok(())
        })();

        match save::open_journal(&self.root) {
            Ok(opened) => {
                self.journal = Some(opened.writer);
                self.recovered = opened.recovered;
                written
            }
            Err(error) => {
                written?;
                Err(error)
            }
        }
    }

    /// Writes the header when it changed.
    pub(super) fn write_header(&mut self) -> Result<(), TerrainError> {
        if self.header_dirty {
            self.header.write(&self.root.join(HEADER_FILE))?;
            self.header_dirty = false;
        }
        Ok(())
    }
}

/// The error for a journal that a trim left closed.
fn closed_journal() -> TerrainError {
    TerrainError::new(
        Stage::Persistence,
        ErrorKind::IoError {
            op: "journal",
            path: PathBuf::from(save::JOURNAL_NAME),
            os_error: 0,
        },
    )
}

/// Writes `description.toml` through a temp file and a rename.
fn write_description(root: &Path, description: &Description) -> Result<(), TerrainError> {
    let path = root.join(DESCRIPTION_FILE);
    let temp = path.with_extension("toml.tmp");
    {
        let mut file = File::create(&temp).map_err(|error| io_error("create", &temp, &error))?;
        file.write_all(description.emit().as_bytes())
            .map_err(|error| io_error("write_all", &temp, &error))?;
        file.sync_all()
            .map_err(|error| io_error("sync_all", &temp, &error))?;
    }
    fs::rename(&temp, &path).map_err(|error| io_error("rename", &temp, &error))?;
    save::sync_directory(root)
}

/// The display-only target tag the header records (`A2.6`).
fn platform_tag() -> String {
    format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}

/// A fresh 128-bit world id, unique within the process and across runs.
fn new_world_id() -> [u8; 16] {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let serial = NEXT_WORLD.fetch_add(1, Ordering::Relaxed);
    let mut seed = Vec::with_capacity(32);
    seed.extend_from_slice(&nanos.to_le_bytes());
    seed.extend_from_slice(&std::process::id().to_le_bytes());
    seed.extend_from_slice(&serial.to_le_bytes());
    let digest = hash::sha256(&seed);
    let mut id = [0_u8; 16];
    id.copy_from_slice(&digest[..16]);
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_save::fixtures::{TempWorld, batch, key, uniform};

    #[test]
    fn create_then_open_round_trips_the_header() {
        let world = TempWorld::new("create");
        let created = WorldSave::create(world.path(), 7, "Test world").expect("the world creates");
        assert!(world.path().join(HEADER_FILE).is_file());
        assert!(world.path().join(DESCRIPTION_FILE).is_file());
        let header = created.header.clone();
        drop(created);

        let opened = WorldSave::open(world.path(), 7).expect("the world opens");
        assert_eq!(opened.header, header);
        assert!(opened.recovered().is_empty());
    }

    #[test]
    fn a_journal_truncated_at_any_byte_resumes() {
        let world = TempWorld::new("journal-fuzz");
        let mut journal = save::open_journal(world.path()).expect("the journal opens");
        for revision in 1..=3 {
            journal
                .writer
                .push(&batch(revision))
                .expect("the batch encodes");
        }
        journal.writer.flush().expect("the journal flushes");
        drop(journal);
        let path = world
            .path()
            .join(save::JOURNAL_DIR)
            .join(save::JOURNAL_NAME);
        let bytes = fs::read(&path).expect("the journal reads");

        for cut in 0..=bytes.len() {
            fs::write(&path, &bytes[..cut]).expect("the prefix writes");
            let reopened = save::open_journal(world.path()).expect("every prefix opens");
            assert!(
                reopened.recovered.len() <= 3,
                "a prefix cannot invent batches"
            );
        }
    }

    #[test]
    fn repeated_crash_reopen_keeps_the_committed_revision() {
        let world = TempWorld::new("crash-resume");
        let stone = content::v1_default()
            .id("core:stone")
            .expect("the v1 pack has stone");
        let section = uniform(stone);
        let mut save = WorldSave::create(world.path(), 7, "Test world").expect("the world creates");

        for revision in 1..=5 {
            save.record_batch(&batch(revision))
                .expect("the batch records");
            save.flush().expect("the journal flushes");
            drop(save);

            let mut reopened =
                WorldSave::open(world.path(), 7).expect("the world reopens after a crash");
            assert_eq!(
                reopened.header.revision,
                revision - 1,
                "a flushed frame is not covered"
            );
            assert_eq!(reopened.recovered().len(), 1);
            reopened
                .save_section(key(0, 0, 0), [1; 32], true, &section, revision)
                .expect("the section covers the frame");
            drop(reopened);

            save = WorldSave::open(world.path(), 7).expect("the world reopens after a save");
            assert_eq!(save.header.revision, revision);
            assert!(save.recovered().is_empty(), "the covered prefix is trimmed");
        }
    }

    #[test]
    fn opening_a_version_1_world_upgrades_the_header() {
        let world = TempWorld::new("upgrade");
        let created = WorldSave::create(world.path(), 7, "Test world").expect("the world creates");
        let header = created.header.clone();
        drop(created);
        fs::write(world.path().join(HEADER_FILE), header.encode_v1())
            .expect("the v1 header writes");

        let opened = WorldSave::open(world.path(), 7).expect("the v1 world opens");
        assert_eq!(opened.header.format_version, header::HEADER_FORMAT_VERSION);
        let reread = Header::read(&world.path().join(HEADER_FILE)).expect("the header rereads");
        assert_eq!(reread.format_version, header::HEADER_FORMAT_VERSION);
        assert_eq!(reread.migration_state, header::MigrationState::None);
    }

    #[test]
    fn a_tampered_description_hash_is_refused() {
        let world = TempWorld::new("hash");
        let opened = WorldSave::create(world.path(), 7, "Test world").expect("the world creates");
        let mut header = opened.header.clone();
        header.description_hash[0] ^= 0xFF;
        header
            .write(&world.path().join(HEADER_FILE))
            .expect("the header writes");
        drop(opened);

        let error = WorldSave::open(world.path(), 7).expect_err("the hash mismatch is refused");
        assert!(matches!(
            error.kind,
            ErrorKind::DescriptionHashMismatch { .. }
        ));
    }

    #[test]
    fn a_different_seed_is_refused() {
        let world = TempWorld::new("seed");
        drop(WorldSave::create(world.path(), 7, "Test world").expect("the world creates"));

        let error = WorldSave::open(world.path(), 8).expect_err("the seed mismatch is refused");
        assert_eq!(
            error.kind,
            ErrorKind::SeedMismatch {
                expected: 8,
                found: 7
            }
        );
    }

    #[test]
    fn saved_sections_survive_reopen_in_key_order() {
        let world = TempWorld::new("sections");
        let mut opened =
            WorldSave::create(world.path(), 7, "Test world").expect("the world creates");
        let stone = content::v1_default()
            .id("core:stone")
            .expect("the v1 pack has stone");
        let dirt = content::v1_default()
            .id("core:dirt")
            .expect("the v1 pack has dirt");
        opened
            .save_section(key(2, -3, 4), [1; 32], true, &uniform(stone), 0)
            .expect("the first section writes");
        opened
            .save_section(key(0, 0, 0), [2; 32], false, &uniform(dirt), 0)
            .expect("the second section writes");
        drop(opened);

        let reopened = WorldSave::open(world.path(), 7).expect("the world reopens");
        let sections = reopened.read_sections().expect("the sections read");
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].key, key(0, 0, 0));
        assert_eq!(sections[1].key, key(2, -3, 4));
        assert!(sections[1].edited);
    }

    #[test]
    fn advance_revision_is_durable() {
        let world = TempWorld::new("advance");
        let mut opened =
            WorldSave::create(world.path(), 7, "Test world").expect("the world creates");
        opened.advance_revision(9).expect("the revision advances");
        drop(opened);

        let reopened = WorldSave::open(world.path(), 7).expect("the world reopens");
        assert_eq!(reopened.header.revision, 9);
    }

    #[test]
    fn a_flush_does_not_mark_a_batch_as_covered() {
        let world = TempWorld::new("revision");
        let mut opened =
            WorldSave::create(world.path(), 7, "Test world").expect("the world creates");
        opened.record_batch(&batch(5)).expect("the batch records");
        opened.flush().expect("the journal flushes");
        drop(opened);

        let reopened = WorldSave::open(world.path(), 7).expect("the world reopens");
        assert_eq!(
            reopened.header.revision, 0,
            "only a section save advances the revision"
        );
        assert_eq!(reopened.recovered().len(), 1);
        assert_eq!(reopened.recovered()[0].revision, 5);
    }

    #[test]
    fn a_saved_section_trims_the_covered_journal_prefix() {
        let world = TempWorld::new("trim");
        let mut opened =
            WorldSave::create(world.path(), 7, "Test world").expect("the world creates");
        let stone = content::v1_default()
            .id("core:stone")
            .expect("the v1 pack has stone");
        opened
            .record_batch(&batch(1))
            .expect("the first batch records");
        opened
            .record_batch(&batch(2))
            .expect("the second batch records");
        opened
            .save_section(key(0, 0, 0), [1; 32], true, &uniform(stone), 1)
            .expect("the first section saves");
        drop(opened);

        let reopened = WorldSave::open(world.path(), 7).expect("the world reopens");
        assert_eq!(reopened.header.revision, 1);
        assert_eq!(reopened.recovered().len(), 1);
        assert_eq!(reopened.recovered()[0].revision, 2);
    }
}
