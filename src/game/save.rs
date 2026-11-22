//! The versioned `VPLY` player save.
//!
//! The player save is a sidecar to the world store. It carries the player's
//! position and view, the inventory and hotbar, and dropped item entities.
//! Stable item names are stored, never numeric ids.

use std::fs;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};

use glam::Vec3;
use thiserror::Error;

use super::entities::EntityRecord;
use super::inventory::{Inventory, TOTAL_SLOTS};
use super::item::ItemRegistry;

/// Player-save magic.
pub(crate) const MAGIC: [u8; 4] = *b"VPLY";
/// Current player-save version.
pub(crate) const VERSION: u16 = 1;
const HEADER_BYTES: usize = 20;
/// Upper bound on dropped entities in one save. A corrupt count must not
/// trigger a huge allocation.
const MAX_ENTITIES: usize = 4096;

/// A player save failure.
#[derive(Debug, Error)]
pub(crate) enum PlayerSaveError {
    #[error("failed to read or write the player save: {0}")]
    Io(#[from] io::Error),
    #[error("not a VPLY player save")]
    BadMagic,
    #[error("unsupported player save version {0}")]
    UnsupportedVersion(u16),
    #[error("player save checksum mismatch")]
    Checksum,
    #[error("player save is truncated or malformed")]
    Malformed,
    #[error("player save names an unknown item: {0}")]
    UnknownItem(String),
}

/// The decoded player state.
#[derive(Clone, Debug)]
pub(crate) struct PlayerSave {
    pub(crate) seed: u64,
    pub(crate) position: Vec3,
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
    pub(crate) inventory: Inventory,
    pub(crate) entities: Vec<EntityRecord>,
}

impl PlayerSave {
    /// Encodes the save, including the header and checksum.
    pub(crate) fn encode(&self, registry: &ItemRegistry) -> Vec<u8> {
        let mut payload = Vec::new();
        put_f32(&mut payload, self.position.x);
        put_f32(&mut payload, self.position.y);
        put_f32(&mut payload, self.position.z);
        put_f32(&mut payload, self.yaw);
        put_f32(&mut payload, self.pitch);
        payload.push(u8::try_from(self.inventory.selected_index()).unwrap_or(0));
        put_u16(&mut payload, u16::try_from(TOTAL_SLOTS).unwrap_or(0));
        for index in 0..TOTAL_SLOTS {
            match self.inventory.slot(index) {
                Some(stack) => {
                    payload.push(1);
                    put_str(&mut payload, registry.name(stack.item));
                    put_u16(&mut payload, stack.count);
                    put_u16(&mut payload, stack.durability);
                }
                None => payload.push(0),
            }
        }
        put_u32(
            &mut payload,
            u32::try_from(self.entities.len()).unwrap_or(u32::MAX),
        );
        for entity in &self.entities {
            put_u64(&mut payload, entity.id);
            put_str(&mut payload, &entity.item);
            put_u16(&mut payload, entity.count);
            put_u16(&mut payload, entity.durability);
            for value in entity.position {
                put_f32(&mut payload, value);
            }
            for value in entity.velocity {
                put_f32(&mut payload, value);
            }
            put_f32(&mut payload, entity.age);
        }

        let mut bytes = Vec::with_capacity(HEADER_BYTES + payload.len());
        bytes.extend_from_slice(&MAGIC);
        put_u16(&mut bytes, VERSION);
        put_u16(&mut bytes, 0);
        put_u64(&mut bytes, self.seed);
        put_u32(&mut bytes, crc32(&payload));
        bytes.extend_from_slice(&payload);
        bytes
    }

    /// Decodes a save and validates its header, version, and checksum.
    pub(crate) fn decode(bytes: &[u8], registry: &ItemRegistry) -> Result<Self, PlayerSaveError> {
        if bytes.len() < HEADER_BYTES || bytes[0..4] != MAGIC {
            return Err(PlayerSaveError::BadMagic);
        }
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if version != VERSION {
            return Err(PlayerSaveError::UnsupportedVersion(version));
        }
        let seed = u64::from_le_bytes(
            bytes[8..16]
                .try_into()
                .map_err(|_| PlayerSaveError::Malformed)?,
        );
        let checksum = u32::from_le_bytes(
            bytes[16..20]
                .try_into()
                .map_err(|_| PlayerSaveError::Malformed)?,
        );
        let payload = &bytes[HEADER_BYTES..];
        if crc32(payload) != checksum {
            return Err(PlayerSaveError::Checksum);
        }

        let mut reader = Reader {
            data: payload,
            pos: 0,
        };
        let position = Vec3::new(reader.f32()?, reader.f32()?, reader.f32()?);
        let yaw = reader.f32()?;
        let pitch = reader.f32()?;
        let selected = usize::from(reader.u8()?);
        let slot_count = usize::from(reader.u16()?);
        if slot_count != TOTAL_SLOTS {
            return Err(PlayerSaveError::Malformed);
        }
        let mut inventory = Inventory::new();
        inventory.select(selected);
        for index in 0..slot_count {
            match reader.u8()? {
                0 => {}
                1 => {
                    let name = reader.string()?;
                    let count = reader.u16()?;
                    let durability = reader.u16()?;
                    let mut stack = registry
                        .stack(&name, count)
                        .ok_or(PlayerSaveError::UnknownItem(name))?;
                    stack.durability = durability;
                    inventory.set_slot(index, Some(stack));
                }
                _ => return Err(PlayerSaveError::Malformed),
            }
        }

        let entity_count =
            usize::try_from(reader.u32()?).map_err(|_| PlayerSaveError::Malformed)?;
        if entity_count > MAX_ENTITIES {
            return Err(PlayerSaveError::Malformed);
        }
        let mut entities = Vec::with_capacity(entity_count);
        for _ in 0..entity_count {
            let id = reader.u64()?;
            let item = reader.string()?;
            let count = reader.u16()?;
            let durability = reader.u16()?;
            let position = [reader.f32()?, reader.f32()?, reader.f32()?];
            let velocity = [reader.f32()?, reader.f32()?, reader.f32()?];
            let age = reader.f32()?;
            entities.push(EntityRecord {
                id,
                item,
                count,
                durability,
                position,
                velocity,
                age,
            });
        }
        if reader.pos != payload.len() {
            return Err(PlayerSaveError::Malformed);
        }

        Ok(Self {
            seed,
            position,
            yaw,
            pitch,
            inventory,
            entities,
        })
    }
}

impl PlayerSave {
    /// Reads a save, or `None` when the file does not exist.
    pub(crate) fn load(
        path: &Path,
        registry: &ItemRegistry,
    ) -> Result<Option<Self>, PlayerSaveError> {
        match fs::read(path) {
            Ok(bytes) => Self::decode(&bytes, registry).map(Some),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(PlayerSaveError::Io(error)),
        }
    }

    /// Writes a save through a temporary file and rename.
    pub(crate) fn write(
        &self,
        path: &Path,
        registry: &ItemRegistry,
    ) -> Result<(), PlayerSaveError> {
        write_atomic(path, &self.encode(registry))
    }
}

/// Moves a corrupt save aside so the next run can start fresh.
pub(crate) fn quarantine(path: &Path) -> io::Result<PathBuf> {
    let bad = path.with_extension("bad");
    if bad.exists() {
        fs::remove_file(&bad)?;
    }
    fs::rename(path, &bad)?;
    Ok(bad)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), PlayerSaveError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tmp");
    {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    match fs::rename(&temporary, path) {
        Ok(()) => {}
        Err(_) if path.exists() => {
            fs::remove_file(path)?;
            fs::rename(&temporary, path)?;
        }
        Err(error) => return Err(PlayerSaveError::Io(error)),
    }
    #[cfg(unix)]
    if let Some(parent) = path.parent()
        && let Ok(directory) = fs::File::open(parent)
    {
        let _ = directory.sync_all();
    }
    Ok(())
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], PlayerSaveError> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or(PlayerSaveError::Malformed)?;
        if end > self.data.len() {
            return Err(PlayerSaveError::Malformed);
        }
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, PlayerSaveError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, PlayerSaveError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, PlayerSaveError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes(
            bytes.try_into().map_err(|_| PlayerSaveError::Malformed)?,
        ))
    }

    fn u64(&mut self) -> Result<u64, PlayerSaveError> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes(
            bytes.try_into().map_err(|_| PlayerSaveError::Malformed)?,
        ))
    }

    fn f32(&mut self) -> Result<f32, PlayerSaveError> {
        let bytes = self.take(4)?;
        let value = f32::from_le_bytes(bytes.try_into().map_err(|_| PlayerSaveError::Malformed)?);
        if value.is_finite() {
            Ok(value)
        } else {
            Err(PlayerSaveError::Malformed)
        }
    }

    fn string(&mut self) -> Result<String, PlayerSaveError> {
        let len = usize::from(self.u16()?);
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| PlayerSaveError::Malformed)
    }
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_f32(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_str(out: &mut Vec<u8>, value: &str) {
    let len = u16::try_from(value.len()).unwrap_or(u16::MAX);
    put_u16(out, len);
    let end = usize::from(len).min(value.len());
    out.extend_from_slice(&value.as_bytes()[..end]);
}

/// IEEE CRC32. The save is small enough that a bitwise loop is fine.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::content;

    use super::*;
    use crate::game::entities::EntityRecord;
    use crate::game::inventory::{Inventory, TOTAL_SLOTS};

    fn registry() -> ItemRegistry {
        ItemRegistry::builtin(&content::v1_default())
    }

    fn temp_path(name: &str) -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        std::env::temp_dir().join(format!(
            "valescent-player-{name}-{}-{}.dat",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn sample(registry: &ItemRegistry) -> PlayerSave {
        let mut inventory = Inventory::new();
        inventory.set_slot(0, registry.stack("core:stone", 12));
        inventory.set_slot(1, registry.stack("core:wooden_pickaxe", 1));
        inventory.select(1);
        PlayerSave {
            seed: 42,
            position: Vec3::new(1.5, 2.0, 3.5),
            yaw: 0.75,
            pitch: -0.25,
            inventory,
            entities: vec![EntityRecord {
                id: 7,
                item: "core:dirt".to_owned(),
                count: 3,
                durability: 0,
                position: [4.0, 5.0, 6.0],
                velocity: [0.1, -0.2, 0.3],
                age: 1.5,
            }],
        }
    }

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn a_save_round_trips_every_field() {
        let registry = registry();
        let save = sample(&registry);
        let bytes = save.encode(&registry);
        let decoded = PlayerSave::decode(&bytes, &registry).unwrap();

        assert_eq!(decoded.seed, 42);
        assert_eq!(decoded.position, save.position);
        assert_eq!(decoded.yaw, save.yaw);
        assert_eq!(decoded.pitch, save.pitch);
        assert_eq!(decoded.inventory, save.inventory);
        assert_eq!(decoded.entities, save.entities);
    }

    #[test]
    fn a_corrupt_payload_is_rejected() {
        let registry = registry();
        let mut bytes = sample(&registry).encode(&registry);
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;

        assert!(matches!(
            PlayerSave::decode(&bytes, &registry),
            Err(PlayerSaveError::Checksum)
        ));
    }

    #[test]
    fn bad_magic_and_versions_are_rejected() {
        let registry = registry();
        let mut bytes = sample(&registry).encode(&registry);
        bytes[0] = b'X';
        assert!(matches!(
            PlayerSave::decode(&bytes, &registry),
            Err(PlayerSaveError::BadMagic)
        ));

        let mut bytes = sample(&registry).encode(&registry);
        bytes[4] = 9;
        assert!(matches!(
            PlayerSave::decode(&bytes, &registry),
            Err(PlayerSaveError::UnsupportedVersion(9))
        ));
    }

    #[test]
    fn an_oversized_entity_count_is_rejected() {
        let registry = registry();
        let mut payload = Vec::new();
        for value in [0.0_f32; 5] {
            put_f32(&mut payload, value);
        }
        payload.push(0);
        put_u16(
            &mut payload,
            u16::try_from(TOTAL_SLOTS).expect("slot count fits"),
        );
        payload.extend(std::iter::repeat_n(0, TOTAL_SLOTS));
        put_u32(&mut payload, u32::MAX);

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        put_u16(&mut bytes, VERSION);
        put_u16(&mut bytes, 0);
        put_u64(&mut bytes, 1);
        put_u32(&mut bytes, crc32(&payload));
        bytes.extend_from_slice(&payload);

        assert!(matches!(
            PlayerSave::decode(&bytes, &registry),
            Err(PlayerSaveError::Malformed)
        ));
    }

    #[test]
    fn save_load_and_quarantine_use_the_file_system() {
        let registry = registry();
        let path = temp_path("round-trip");
        let save = sample(&registry);
        save.write(&path, &registry).unwrap();

        let loaded = PlayerSave::load(&path, &registry).unwrap().unwrap();
        assert_eq!(loaded.seed, 42);
        assert_eq!(loaded.inventory.slot(0).unwrap().count, 12);

        let bad = quarantine(&path).unwrap();
        assert!(!path.exists());
        assert!(bad.exists());
        let _ = fs::remove_file(bad);
    }
}
