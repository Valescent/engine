//! Migration journal records: the append-only progress log (`A16`).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use crate::error::{SectionFault, TerrainError};
use crate::section::SectionKey;

use super::{corrupt, io_error};

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
/// [`crate::error::ErrorKind::IoError`] for a filesystem failure and
/// [`crate::error::ErrorKind::CorruptSection`] for a bad complete record.
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
/// [`crate::error::ErrorKind::IoError`] for a filesystem failure.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::save::fixtures::{TempWorld, key};

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
}
