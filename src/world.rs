//! Chunked block storage, dirty-chunk tracking, and `VCHK` persistence.
//!
//! Chunks are `32 × 32 × 32` cubes. A world coordinate is split into a chunk
//! coordinate and a local coordinate with Euclidean division, so negative
//! worlds map to negative chunks without truncation.

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use glam::{IVec3, Vec3};
use thiserror::Error;

use crate::block::Block;
use crate::light::{self, LightField};

/// Edge length of a chunk on every axis.
pub(crate) const CHUNK_SIZE: i32 = 32;

/// Cells in one chunk.
pub(crate) const CHUNK_CELLS: usize = (CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE) as usize;

/// Chunks touched by an edit, in deterministic order.
pub(crate) type DirtyChunks = BTreeSet<ChunkCoord>;

const MAGIC: [u8; 4] = *b"VCHK";
const FORMAT_VERSION: u16 = 3;
const OLDEST_FORMAT_VERSION: u16 = 1;
const V1_MAX_BLOCK_ID: u8 = 5;
const V2_MAX_BLOCK_ID: u8 = 7;
const HEADER_BYTES: usize = 22;
const RECORD_BYTES: usize = 4;
const MAX_BLOCK_COUNT: u32 = 32_768;
const CHUNK_FILE_PREFIX: &str = "chunk_";
const CHUNK_FILE_SUFFIX: &str = ".vchk";
const SEED_FILE: &str = "seed";

/// World directory used by the streaming world: `world/`.
pub(crate) const WORLD_DIRECTORY: &str = "world";

/// Coordinate of one chunk in chunk space.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ChunkCoord(IVec3);

impl ChunkCoord {
    /// Splits a world coordinate into its chunk and local coordinate.
    pub(crate) fn from_world(world: IVec3) -> (Self, IVec3) {
        let size = IVec3::splat(CHUNK_SIZE);
        (Self(world.div_euclid(size)), world.rem_euclid(size))
    }

    /// Builds a chunk coordinate from chunk-space axes.
    pub(crate) fn new(x: i32, y: i32, z: i32) -> Self {
        Self(IVec3::new(x, y, z))
    }

    /// Minimum-corner world coordinate of the chunk.
    pub(crate) fn world_origin(self) -> IVec3 {
        self.0 * CHUNK_SIZE
    }

    /// Minimum-corner world position in metres.
    pub(crate) fn origin(self) -> Vec3 {
        self.world_origin().as_vec3()
    }

    /// Chunk-space coordinate vector.
    pub(crate) fn coordinates(self) -> IVec3 {
        self.0
    }

    pub(crate) fn offset(self, by: IVec3) -> Self {
        Self(self.0 + by)
    }
}

impl Ord for ChunkCoord {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.0.x, self.0.y, self.0.z).cmp(&(other.0.x, other.0.y, other.0.z))
    }
}

impl PartialOrd for ChunkCoord {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Blocks owned by one chunk, stored in local coordinates.
///
/// Light is derived state: it is recomputed from blocks and never saved. Both
/// arrays hold one `0..=15` value per cell in local coordinates.
#[derive(Debug)]
pub(crate) struct Chunk {
    pub(crate) blocks: HashMap<IVec3, Block>,
    pub(crate) sky_light: Box<[u8]>,
    pub(crate) block_light: Box<[u8]>,
}

impl Default for Chunk {
    fn default() -> Self {
        Self {
            blocks: HashMap::new(),
            sky_light: vec![0; CHUNK_CELLS].into_boxed_slice(),
            block_light: vec![0; CHUNK_CELLS].into_boxed_slice(),
        }
    }
}

impl Chunk {
    fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Returns `(sky, block)` light at a local coordinate.
    #[cfg(test)]
    pub(crate) fn light(&self, local: IVec3) -> (u8, u8) {
        let index = local_index(local);
        (self.sky_light[index], self.block_light[index])
    }

    /// Stores `(sky, block)` light at a local coordinate.
    pub(crate) fn set_light(&mut self, local: IVec3, sky: u8, block: u8) {
        let index = local_index(local);
        self.sky_light[index] = sky;
        self.block_light[index] = block;
    }
}

/// Linear index of a chunk-local coordinate.
pub(crate) fn local_index(local: IVec3) -> usize {
    let stride = usize::try_from(CHUNK_SIZE).expect("chunk size fits");
    let x = usize::try_from(local.x).expect("local coordinate fits");
    let y = usize::try_from(local.y).expect("local coordinate fits");
    let z = usize::try_from(local.z).expect("local coordinate fits");
    (x * stride + y) * stride + z
}

/// Chunked block world.
///
/// Missing chunks are air, and every stored chunk is non-empty.
#[derive(Debug, Default)]
pub(crate) struct World {
    chunks: HashMap<ChunkCoord, Chunk>,
    light: HashMap<ChunkCoord, Arc<LightField>>,
    modified: HashSet<ChunkCoord>,
}

impl World {
    /// Returns the block at a world coordinate, or `Air` when missing.
    pub(crate) fn block(&self, world: IVec3) -> Block {
        let (chunk, local) = ChunkCoord::from_world(world);
        self.chunks
            .get(&chunk)
            .and_then(|chunk_data| chunk_data.blocks.get(&local))
            .copied()
            .unwrap_or(Block::Air)
    }

    /// Sets a world coordinate and returns the chunks whose meshes changed.
    pub(crate) fn set_block(&mut self, world: IVec3, block: Block) -> DirtyChunks {
        let current = self.block(world);
        if current == block {
            return DirtyChunks::new();
        }

        let (chunk, local) = ChunkCoord::from_world(world);
        let dirty = if current.is_solid() == block.is_solid() {
            DirtyChunks::from([chunk])
        } else {
            touched_chunks(chunk, local)
        };

        self.modified.insert(chunk);

        if block == Block::Air {
            let Some(chunk_data) = self.chunks.get_mut(&chunk) else {
                return DirtyChunks::new();
            };
            chunk_data.blocks.remove(&local);
            if chunk_data.is_empty() {
                self.chunks.remove(&chunk);
                self.light.remove(&chunk);
            }
        } else {
            self.chunks
                .entry(chunk)
                .or_default()
                .blocks
                .insert(local, block);
        }
        dirty
    }

    /// Returns whether a world coordinate holds a solid block.
    pub(crate) fn is_solid(&self, world: IVec3) -> bool {
        self.block(world).is_solid()
    }

    /// Returns a stored chunk, or `None` when the chunk is empty.
    pub(crate) fn chunk(&self, chunk: ChunkCoord) -> Option<&Chunk> {
        self.chunks.get(&chunk)
    }

    /// Inserts a generated or loaded chunk. Empty block maps are ignored.
    pub(crate) fn insert_chunk(&mut self, chunk: ChunkCoord, blocks: HashMap<IVec3, Block>) {
        if blocks.is_empty() {
            return;
        }
        self.chunks.insert(
            chunk,
            Chunk {
                blocks,
                ..Chunk::default()
            },
        );
    }

    /// Drops one chunk from memory, keeping its modified marker.
    pub(crate) fn remove_chunk(&mut self, chunk: ChunkCoord) {
        self.chunks.remove(&chunk);
        self.light.remove(&chunk);
    }

    /// Iterates over every non-empty chunk in unspecified order.
    pub(crate) fn iter_chunks(&self) -> impl Iterator<Item = (ChunkCoord, &Chunk)> {
        self.chunks
            .iter()
            .map(|(coordinate, chunk)| (*coordinate, chunk))
    }

    /// Chunks affected by an edit, including light propagation and covered columns.
    pub(crate) fn light_dirty(&self, dirty: &DirtyChunks) -> DirtyChunks {
        let mut result = DirtyChunks::new();
        for chunk in dirty {
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        let candidate = chunk.offset(IVec3::new(dx, dy, dz));
                        if self.chunks.contains_key(&candidate) {
                            result.insert(candidate);
                        }
                    }
                }
            }
            for candidate in self.chunks.keys() {
                if candidate.0.x == chunk.0.x
                    && candidate.0.z == chunk.0.z
                    && candidate.0.y < chunk.0.y
                {
                    result.insert(*candidate);
                }
            }
        }
        result
    }

    /// Stores an accepted light field and its centre in the chunk arrays.
    pub(crate) fn set_light_field(&mut self, chunk: ChunkCoord, field: LightField) {
        if let Some(data) = self.chunks.get_mut(&chunk) {
            for x in 0..CHUNK_SIZE {
                for y in 0..CHUNK_SIZE {
                    for z in 0..CHUNK_SIZE {
                        let local = IVec3::new(x, y, z);
                        data.set_light(local, field.sky(local), field.block(local));
                    }
                }
            }
        }
        self.light.insert(chunk, Arc::new(field));
    }

    /// Light field for a chunk, when light has been computed.
    pub(crate) fn light_field(&self, chunk: ChunkCoord) -> Option<&Arc<LightField>> {
        self.light.get(&chunk)
    }

    /// Recomputes sky and block light for every stored chunk.
    ///
    /// Light is derived state, so this runs after loading and whenever a world
    /// is built in memory.
    pub(crate) fn relight(&mut self) {
        let fields: Vec<(ChunkCoord, Arc<LightField>)> = self
            .chunks
            .keys()
            .map(|&chunk| {
                let snapshot = light::LightSnapshot::new(self, chunk);
                (chunk, Arc::new(light::compute(&snapshot, chunk)))
            })
            .collect();
        self.light.clear();
        for (chunk, field) in fields {
            self.light.insert(chunk, Arc::clone(&field));
            let Some(data) = self.chunks.get_mut(&chunk) else {
                continue;
            };
            for x in 0..CHUNK_SIZE {
                for y in 0..CHUNK_SIZE {
                    for z in 0..CHUNK_SIZE {
                        let local = IVec3::new(x, y, z);
                        data.set_light(local, field.sky(local), field.block(local));
                    }
                }
            }
        }
    }

    /// Returns every solid block inside the inclusive world-coordinate bounds.
    ///
    /// Queries the box directly instead of scanning stored chunks, so the cost
    /// scales with the requested volume rather than with the world size.
    pub(crate) fn solid_blocks_in(&self, min: IVec3, max: IVec3) -> Vec<IVec3> {
        let mut blocks = Vec::new();
        if min.cmpgt(max).any() {
            return blocks;
        }
        for x in min.x..=max.x {
            for y in min.y..=max.y {
                for z in min.z..=max.z {
                    let world = IVec3::new(x, y, z);
                    if self.is_solid(world) {
                        blocks.push(world);
                    }
                }
            }
        }
        blocks
    }

    /// Loads every `.vchk` file in a directory.
    ///
    /// A missing directory is an empty world. Any malformed file aborts the
    /// load without returning a partial world.
    #[cfg(test)]
    pub(crate) fn load_chunks(directory: &Path) -> Result<Self, SaveError> {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(SaveError::from_path(&error, directory)),
        };

        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| SaveError::from_path(&error, directory))?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().is_none_or(|extension| extension != "vchk") {
                continue;
            }
            paths.push(path);
        }
        paths.sort();

        let mut world = Self::default();
        let mut seen_chunks = HashSet::new();
        for path in paths {
            let file_name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let file_chunk = parse_chunk_file_name(&file_name)
                .ok_or_else(|| SaveError::InvalidFileName(file_name.clone()))?;
            let bytes = fs::read(&path).map_err(|error| SaveError::from_path(&error, &path))?;
            let (header_chunk, chunk) = parse_chunk(&bytes)?;

            if !seen_chunks.insert(header_chunk) {
                return Err(SaveError::DuplicateChunk(header_chunk));
            }
            if header_chunk != file_chunk {
                return Err(SaveError::ChunkMismatch {
                    file: file_chunk,
                    header: header_chunk,
                });
            }
            world.modified.insert(header_chunk);
            if !chunk.blocks.is_empty() {
                world.chunks.insert(header_chunk, chunk);
            }
        }
        world.relight();
        Ok(world)
    }

    /// Writes every modified chunk.
    ///
    /// Unmodified chunks have no file and are regenerated from the seed. Empty
    /// modified chunks are written as v3 files with zero records.
    pub(crate) fn save_chunks(&self, directory: &Path) -> Result<(), SaveError> {
        fs::create_dir_all(directory).map_err(|error| SaveError::from_path(&error, directory))?;

        let mut modified: Vec<_> = self.modified.iter().copied().collect();
        modified.sort();
        for coord in modified {
            let bytes = match self.chunks.get(&coord) {
                Some(chunk) => serialize_chunk(coord, chunk),
                None => serialize_empty_chunk(coord),
            };
            let path = chunk_file_path(directory, coord);
            fs::write(&path, bytes).map_err(|error| SaveError::from_path(&error, &path))?;
        }
        Ok(())
    }

    /// Writes one modified chunk and clears its marker.
    pub(crate) fn save_chunk(
        &mut self,
        directory: &Path,
        chunk: ChunkCoord,
    ) -> Result<(), SaveError> {
        if !self.modified.contains(&chunk) {
            return Ok(());
        }
        fs::create_dir_all(directory).map_err(|error| SaveError::from_path(&error, directory))?;
        let bytes = match self.chunks.get(&chunk) {
            Some(data) => serialize_chunk(chunk, data),
            None => serialize_empty_chunk(chunk),
        };
        let path = chunk_file_path(directory, chunk);
        fs::write(&path, bytes).map_err(|error| SaveError::from_path(&error, &path))?;
        self.modified.remove(&chunk);
        Ok(())
    }

    /// Reads one saved chunk, if a file exists.
    pub(crate) fn load_saved_chunk(
        directory: &Path,
        chunk: ChunkCoord,
    ) -> Result<Option<Chunk>, SaveError> {
        let path = chunk_file_path(directory, chunk);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(SaveError::from_path(&error, &path)),
        };
        let (header, data) = parse_chunk(&bytes)?;
        if header != chunk {
            return Err(SaveError::ChunkMismatch {
                file: chunk,
                header,
            });
        }
        Ok(Some(data))
    }

    /// Marks a loaded chunk as modified.
    pub(crate) fn mark_modified(&mut self, chunk: ChunkCoord) {
        self.modified.insert(chunk);
    }

    /// Whether a chunk has unsaved edits.
    #[cfg(test)]
    pub(crate) fn is_modified(&self, chunk: ChunkCoord) -> bool {
        self.modified.contains(&chunk)
    }
}

fn touched_chunks(chunk: ChunkCoord, local: IVec3) -> DirtyChunks {
    let mut dirty = DirtyChunks::from([chunk]);
    for axis in [IVec3::X, IVec3::Y, IVec3::Z] {
        match local.dot(axis) {
            0 => {
                dirty.insert(chunk.offset(-axis));
            }
            value if value == CHUNK_SIZE - 1 => {
                dirty.insert(chunk.offset(axis));
            }
            _ => {}
        }
    }
    dirty
}

/// Persistence failures for chunk load and save.
#[derive(Debug, Error)]
pub enum SaveError {
    #[error("chunk I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("invalid chunk file name: {0}")]
    InvalidFileName(String),
    #[error("invalid chunk file magic")]
    InvalidMagic,
    #[error("unsupported chunk format version: {0}")]
    UnsupportedVersion(u16),
    #[error("chunk coordinate does not fit in i32: {0:?}")]
    InvalidChunkCoordinate(IVec3),
    #[error("invalid block count: {0}")]
    InvalidBlockCount(u32),
    #[error("invalid local coordinate: {0:?}")]
    InvalidLocalCoordinate(IVec3),
    #[error("invalid block ID: {0}")]
    InvalidBlockId(u8),
    #[error("duplicate block record: {0:?}")]
    DuplicateRecord(IVec3),
    #[error("duplicate chunk file for {0:?}")]
    DuplicateChunk(ChunkCoord),
    #[error("chunk file {file:?} does not match header chunk {header:?}")]
    ChunkMismatch {
        file: ChunkCoord,
        header: ChunkCoord,
    },
    #[error("chunk file is truncated")]
    Truncated,
    #[error("chunk file has trailing bytes")]
    TrailingBytes,
    #[error("world/seed is missing but chunk files exist")]
    MissingSeed,
    #[error("invalid world seed file: {0:?}")]
    InvalidSeed(String),
}

impl SaveError {
    fn from_path(error: &io::Error, path: &Path) -> Self {
        Self::Io(io::Error::new(
            error.kind(),
            format!("{}: {error}", path.display()),
        ))
    }
}

fn chunk_file_path(directory: &Path, chunk: ChunkCoord) -> PathBuf {
    directory.join(format!(
        "{CHUNK_FILE_PREFIX}{}_{}_{}{CHUNK_FILE_SUFFIX}",
        chunk.0.x, chunk.0.y, chunk.0.z
    ))
}

#[cfg(test)]
fn parse_chunk_file_name(name: &str) -> Option<ChunkCoord> {
    let stem = name
        .strip_prefix(CHUNK_FILE_PREFIX)?
        .strip_suffix(CHUNK_FILE_SUFFIX)?;
    let mut parts = stem.split('_');
    let x = parts.next()?.parse().ok()?;
    let y = parts.next()?.parse().ok()?;
    let z = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(ChunkCoord(IVec3::new(x, y, z)))
}

fn chunk_origin_checked(chunk: ChunkCoord) -> Option<IVec3> {
    Some(IVec3::new(
        chunk.0.x.checked_mul(CHUNK_SIZE)?,
        chunk.0.y.checked_mul(CHUNK_SIZE)?,
        chunk.0.z.checked_mul(CHUNK_SIZE)?,
    ))
}

fn parse_chunk(bytes: &[u8]) -> Result<(ChunkCoord, Chunk), SaveError> {
    if bytes.len() < MAGIC.len() {
        return Err(SaveError::Truncated);
    }
    if !bytes.starts_with(&MAGIC) {
        return Err(SaveError::InvalidMagic);
    }
    if bytes.len() < 6 {
        return Err(SaveError::Truncated);
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if !(OLDEST_FORMAT_VERSION..=FORMAT_VERSION).contains(&version) {
        return Err(SaveError::UnsupportedVersion(version));
    }
    if bytes.len() < HEADER_BYTES {
        return Err(SaveError::Truncated);
    }

    let header_chunk = ChunkCoord(IVec3::new(
        read_i32(bytes, 6).expect("header length checked"),
        read_i32(bytes, 10).expect("header length checked"),
        read_i32(bytes, 14).expect("header length checked"),
    ));
    if chunk_origin_checked(header_chunk).is_none() {
        return Err(SaveError::InvalidChunkCoordinate(header_chunk.0));
    }

    let block_count = read_u32(bytes, 18).expect("header length checked");
    let minimum_count = u32::from(version < 3);
    if block_count < minimum_count || block_count > MAX_BLOCK_COUNT {
        return Err(SaveError::InvalidBlockCount(block_count));
    }
    let block_count_usize = usize::try_from(block_count).expect("block count fits in usize");
    let expected_size = HEADER_BYTES + block_count_usize * RECORD_BYTES;
    if bytes.len() < expected_size {
        return Err(SaveError::Truncated);
    }
    if bytes.len() > expected_size {
        return Err(SaveError::TrailingBytes);
    }

    let mut blocks = HashMap::with_capacity(block_count_usize);
    for record in 0..block_count_usize {
        let offset = HEADER_BYTES + record * RECORD_BYTES;
        let local = IVec3::new(
            i32::from(bytes[offset]),
            i32::from(bytes[offset + 1]),
            i32::from(bytes[offset + 2]),
        );
        if !(local.cmpge(IVec3::ZERO).all() && local.cmplt(IVec3::splat(CHUNK_SIZE)).all()) {
            return Err(SaveError::InvalidLocalCoordinate(local));
        }

        let block_id = bytes[offset + 3];
        let max_id = match version {
            1 => V1_MAX_BLOCK_ID,
            2 => V2_MAX_BLOCK_ID,
            _ => u8::MAX,
        };
        let block = Block::from_id(block_id)
            .filter(|block| *block != Block::Air && block_id <= max_id)
            .ok_or(SaveError::InvalidBlockId(block_id))?;
        if blocks.insert(local, block).is_some() {
            return Err(SaveError::DuplicateRecord(local));
        }
    }

    Ok((
        header_chunk,
        Chunk {
            blocks,
            ..Chunk::default()
        },
    ))
}

fn serialize_chunk(chunk_coord: ChunkCoord, chunk: &Chunk) -> Vec<u8> {
    let mut records: Vec<_> = chunk.blocks.iter().collect();
    records.sort_by_key(|(local, _)| (local.x, local.y, local.z));

    let block_count = u32::try_from(records.len()).expect("chunk block count fits in u32");
    let mut bytes = serialize_header(chunk_coord, block_count);
    for (local, block) in records {
        bytes.push(u8::try_from(local.x).expect("local coordinate fits in u8"));
        bytes.push(u8::try_from(local.y).expect("local coordinate fits in u8"));
        bytes.push(u8::try_from(local.z).expect("local coordinate fits in u8"));
        bytes.push(block.id());
    }
    bytes
}

fn serialize_empty_chunk(chunk_coord: ChunkCoord) -> Vec<u8> {
    serialize_header(chunk_coord, 0)
}

fn serialize_header(chunk_coord: ChunkCoord, block_count: u32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(
        HEADER_BYTES
            + usize::try_from(block_count).expect("block count fits in usize") * RECORD_BYTES,
    );
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&chunk_coord.0.x.to_le_bytes());
    bytes.extend_from_slice(&chunk_coord.0.y.to_le_bytes());
    bytes.extend_from_slice(&chunk_coord.0.z.to_le_bytes());
    bytes.extend_from_slice(&block_count.to_le_bytes());
    bytes
}

/// Reads or creates `world/seed`.
pub(crate) fn load_or_create_seed(
    world_directory: &Path,
    preferred: Option<u64>,
) -> Result<u64, SaveError> {
    let seed_path = world_directory.join(SEED_FILE);
    match fs::read_to_string(&seed_path) {
        Ok(text) => parse_seed(&text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if directory_has_chunks(&world_directory.join("chunks"))? {
                return Err(SaveError::MissingSeed);
            }
            let seed = preferred.unwrap_or_else(fresh_seed);
            fs::create_dir_all(world_directory)
                .map_err(|error| SaveError::from_path(&error, world_directory))?;
            fs::write(&seed_path, format!("{seed}\n"))
                .map_err(|error| SaveError::from_path(&error, &seed_path))?;
            Ok(seed)
        }
        Err(error) => Err(SaveError::from_path(&error, &seed_path)),
    }
}

fn parse_seed(text: &str) -> Result<u64, SaveError> {
    let trimmed = text.strip_suffix('\n').unwrap_or(text);
    let trimmed = trimmed.strip_suffix('\r').unwrap_or(trimmed);
    if trimmed.is_empty() || !trimmed.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(SaveError::InvalidSeed(text.to_owned()));
    }
    trimmed
        .parse()
        .map_err(|_| SaveError::InvalidSeed(text.to_owned()))
}

fn directory_has_chunks(directory: &Path) -> Result<bool, SaveError> {
    match fs::read_dir(directory) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry.map_err(|error| SaveError::from_path(&error, directory))?;
                let path = entry.path();
                if path.is_file()
                    && path
                        .extension()
                        .is_some_and(|extension| extension == "vchk")
                {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(SaveError::from_path(&error, directory)),
    }
}

#[allow(clippy::cast_possible_truncation)]
fn fresh_seed() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos() as u64);
    crate::generation::splitmix64(nanos ^ u64::from(std::process::id()))
}

fn read_i32(bytes: &[u8], offset: usize) -> Option<i32> {
    Some(i32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    fn chunk_at(world: IVec3) -> ChunkCoord {
        ChunkCoord::from_world(world).0
    }

    fn dirty(coordinates: &[IVec3]) -> DirtyChunks {
        coordinates.iter().copied().map(chunk_at).collect()
    }

    fn temp_dir(name: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let directory = std::env::temp_dir().join(format!(
            "valescent-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn write_raw(directory: &Path, name: &str, bytes: &[u8]) {
        fs::write(directory.join(name), bytes).unwrap();
    }

    fn header(chunk: ChunkCoord, count: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&chunk.0.x.to_le_bytes());
        bytes.extend_from_slice(&chunk.0.y.to_le_bytes());
        bytes.extend_from_slice(&chunk.0.z.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes
    }

    #[test]
    fn world_to_chunk_and_local_matches_the_table() {
        let cases = [
            (
                IVec3::new(-33, 0, 0),
                IVec3::new(-2, 0, 0),
                IVec3::new(31, 0, 0),
            ),
            (
                IVec3::new(-32, 0, 0),
                IVec3::new(-1, 0, 0),
                IVec3::new(0, 0, 0),
            ),
            (
                IVec3::new(-1, 0, 0),
                IVec3::new(-1, 0, 0),
                IVec3::new(31, 0, 0),
            ),
            (IVec3::ZERO, IVec3::ZERO, IVec3::ZERO),
            (IVec3::new(31, 0, 0), IVec3::ZERO, IVec3::new(31, 0, 0)),
            (IVec3::new(32, 0, 0), IVec3::new(1, 0, 0), IVec3::ZERO),
        ];

        for (world, chunk, local) in cases {
            let (actual_chunk, actual_local) = ChunkCoord::from_world(world);
            assert_eq!(actual_chunk.0, chunk);
            assert_eq!(actual_local, local);
        }
    }

    #[test]
    fn every_world_coordinate_round_trips() {
        for world in [
            IVec3::MIN,
            IVec3::MAX,
            IVec3::new(i32::MIN, 0, i32::MAX),
            IVec3::new(-31, -1, 31),
            IVec3::new(65_536, -65_536, 0),
        ] {
            let (chunk, local) = ChunkCoord::from_world(world);
            assert_eq!(chunk.world_origin() + local, world);
            assert!(local.cmpge(IVec3::ZERO).all() && local.cmplt(IVec3::splat(CHUNK_SIZE)).all());
        }
    }

    #[test]
    fn chunks_order_lexicographically() {
        let mut chunks = [
            chunk_at(IVec3::new(0, 0, 1)),
            chunk_at(IVec3::new(1, 0, 0)),
            chunk_at(IVec3::new(0, 1, 0)),
            chunk_at(IVec3::new(0, 0, 0)),
        ];
        chunks.sort();

        let expected = [
            chunk_at(IVec3::new(0, 0, 0)),
            chunk_at(IVec3::new(0, 0, 1)),
            chunk_at(IVec3::new(0, 1, 0)),
            chunk_at(IVec3::new(1, 0, 0)),
        ];
        assert_eq!(chunks, expected);
    }

    #[test]
    fn origin_is_the_chunk_minimum_corner() {
        let chunk = chunk_at(IVec3::new(-32, 64, 0));

        assert_eq!(chunk.world_origin(), IVec3::new(-32, 64, 0));
        assert_eq!(chunk.origin(), Vec3::new(-32.0, 64.0, 0.0));
    }

    #[test]
    fn missing_chunks_are_air() {
        let world = World::default();

        assert_eq!(world.block(IVec3::ZERO), Block::Air);
        assert!(!world.is_solid(IVec3::new(-1, -1, -1)));
    }

    #[test]
    fn set_block_is_idempotent() {
        let mut world = World::default();
        let interior = IVec3::new(5, 5, 5);

        assert_eq!(world.set_block(interior, Block::Stone), dirty(&[interior]));
        assert!(world.set_block(interior, Block::Stone).is_empty());
        assert_eq!(world.block(interior), Block::Stone);
    }

    #[test]
    fn setting_air_on_air_is_a_no_op() {
        let mut world = World::default();

        assert!(world.set_block(IVec3::ZERO, Block::Air).is_empty());
        assert!(world.iter_chunks().next().is_none());
    }

    #[test]
    fn solid_to_solid_dirties_only_the_owner() {
        let mut world = World::default();
        let border = IVec3::new(31, 5, 5);
        world.set_block(border, Block::Stone);

        assert_eq!(world.set_block(border, Block::Dirt), dirty(&[border]));
    }

    #[test]
    fn border_edits_dirty_touched_neighbours_on_solidity_change() {
        let mut world = World::default();
        let border = IVec3::new(31, 5, 5);

        assert_eq!(
            world.set_block(border, Block::Stone),
            dirty(&[border, IVec3::new(32, 5, 5)])
        );
    }

    #[test]
    fn corner_edits_dirty_three_neighbours() {
        let mut world = World::default();
        let corner = IVec3::new(0, 31, 0);

        let expected = dirty(&[
            corner,
            IVec3::new(-1, 31, 0),
            IVec3::new(0, 32, 0),
            IVec3::new(0, 31, -1),
        ]);
        assert_eq!(world.set_block(corner, Block::Wood), expected);
    }

    #[test]
    fn removing_the_last_block_deletes_the_chunk_and_dirties_neighbours() {
        let mut world = World::default();
        let border = IVec3::new(32, 0, 0);
        world.set_block(border, Block::Stone);

        let removed = world.set_block(border, Block::Air);

        assert!(world.chunk(chunk_at(border)).is_none());
        assert_eq!(
            removed,
            dirty(&[
                border,
                IVec3::new(31, 0, 0),
                IVec3::new(32, 0, -1),
                IVec3::new(32, -1, 0)
            ])
        );
    }

    #[test]
    fn stored_chunks_never_go_empty() {
        let mut world = World::default();
        let a = IVec3::new(0, 0, 0);
        let b = IVec3::new(1, 0, 0);
        world.set_block(a, Block::Stone);
        world.set_block(b, Block::Dirt);
        assert_eq!(world.iter_chunks().count(), 1);

        world.set_block(a, Block::Air);
        assert_eq!(world.iter_chunks().count(), 1);
        world.set_block(b, Block::Air);
        assert_eq!(world.iter_chunks().count(), 0);
    }

    #[test]
    fn light_dirty_covers_neighbours_and_columns_below() {
        let mut world = World::default();
        let chunks = [
            IVec3::new(0, 0, 0),
            IVec3::new(1, 0, 0),
            IVec3::new(0, 0, 1),
            IVec3::new(0, 1, 0),
            IVec3::new(0, -1, 0),
            IVec3::new(2, 0, 0),
            IVec3::new(0, 0, 2),
        ];
        for origin in chunks {
            world.set_block(origin * CHUNK_SIZE, Block::Stone);
        }

        let dirty = DirtyChunks::from([chunk_at(IVec3::new(0, 32, 0))]);
        let light = world.light_dirty(&dirty);

        assert!(light.contains(&chunk_at(IVec3::new(0, 32, 0))));
        assert!(light.contains(&chunk_at(IVec3::new(32, 0, 0))));
        assert!(light.contains(&chunk_at(IVec3::new(0, 0, 32))));
        assert!(light.contains(&chunk_at(IVec3::new(0, 0, 0))));
        assert!(light.contains(&chunk_at(IVec3::new(0, -32, 0))));
        assert!(!light.contains(&chunk_at(IVec3::new(64, 0, 0))));
        assert!(!light.contains(&chunk_at(IVec3::new(0, 0, 64))));
    }

    #[test]
    fn solid_blocks_in_matches_the_requested_box_across_chunks() {
        let mut world = World::default();
        let solids = [
            IVec3::new(-1, 0, 0),
            IVec3::new(0, 0, 0),
            IVec3::new(31, 2, 0),
            IVec3::new(32, 2, 0),
            IVec3::new(33, 2, 0),
        ];
        for coordinate in solids {
            world.set_block(coordinate, Block::Stone);
        }

        let mut found = world.solid_blocks_in(IVec3::new(-1, 0, 0), IVec3::new(32, 2, 0));
        found.sort_by_key(|block| (block.x, block.y, block.z));

        let mut expected = vec![
            IVec3::new(-1, 0, 0),
            IVec3::new(0, 0, 0),
            IVec3::new(31, 2, 0),
            IVec3::new(32, 2, 0),
        ];
        expected.sort_by_key(|block| (block.x, block.y, block.z));
        assert_eq!(found, expected);
    }

    #[test]
    fn solid_blocks_in_returns_nothing_for_empty_or_inverted_boxes() {
        let world = World::default();
        assert_eq!(
            world.solid_blocks_in(IVec3::ZERO, IVec3::splat(4)),
            Vec::new()
        );

        let mut world = World::default();
        world.set_block(IVec3::new(0, 0, 0), Block::Stone);
        assert_eq!(
            world.solid_blocks_in(IVec3::new(1, 1, 1), IVec3::ZERO),
            Vec::new()
        );
        assert_eq!(
            world.solid_blocks_in(IVec3::new(1, 0, 0), IVec3::new(2, 2, 2)),
            Vec::new()
        );
    }

    #[test]
    fn stored_chunks_never_contain_air() {
        let mut world = World::default();
        world.set_block(IVec3::new(5, 5, 5), Block::Stone);
        world.set_block(IVec3::new(6, 5, 5), Block::Dirt);
        world.set_block(IVec3::new(6, 5, 5), Block::Air);

        for (_, chunk) in world.iter_chunks() {
            assert!(chunk.blocks.values().all(|block| *block != Block::Air));
        }
    }

    #[test]
    fn chunk_lookup_matches_storage() {
        let mut world = World::default();
        world.set_block(IVec3::new(32, 0, 0), Block::Grass);

        let stored = world.chunk(chunk_at(IVec3::new(32, 0, 0))).unwrap();
        assert_eq!(stored.blocks.get(&IVec3::ZERO), Some(&Block::Grass));
        assert!(world.chunk(chunk_at(IVec3::ZERO)).is_none());
    }

    #[test]
    fn save_and_load_round_trip_negative_chunks_and_borders() {
        let directory = temp_dir("round-trip");
        let mut world = World::default();
        let coordinates = [
            (IVec3::new(-33, 0, 0), Block::Stone),
            (IVec3::new(-32, 31, 31), Block::Grass),
            (IVec3::new(0, 0, 0), Block::Dirt),
            (IVec3::new(31, 5, 31), Block::Leaves),
            (IVec3::new(32, 1, 32), Block::Wood),
        ];
        for (coordinate, block) in coordinates {
            world.set_block(coordinate, block);
        }

        world.save_chunks(&directory).unwrap();
        let loaded = World::load_chunks(&directory).unwrap();

        for (coordinate, block) in coordinates {
            assert_eq!(loaded.block(coordinate), block, "{coordinate}");
        }
        assert_eq!(loaded.iter_chunks().count(), 4);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn missing_directory_loads_an_empty_world() {
        let directory = temp_dir("missing").join("nested");
        let world = World::load_chunks(&directory).unwrap();
        assert!(world.iter_chunks().next().is_none());
        fs::remove_dir_all(directory.parent().unwrap()).unwrap();
    }

    #[test]
    fn empty_directory_loads_an_empty_world() {
        let directory = temp_dir("empty");
        let world = World::load_chunks(&directory).unwrap();
        assert!(world.iter_chunks().next().is_none());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn saving_leaves_unmodified_chunk_files_alone() {
        let directory = temp_dir("modified-only");
        let mut world = World::default();
        world.set_block(IVec3::new(5, 0, 5), Block::Stone);
        world.save_chunks(&directory).unwrap();
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);

        World::default().save_chunks(&directory).unwrap();

        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn saving_writes_only_modified_chunks() {
        let directory = temp_dir("saves-modified");
        let mut world = World::default();
        world.set_block(IVec3::new(5, 0, 5), Block::Stone);
        world.save_chunks(&directory).unwrap();
        world.set_block(IVec3::new(37, 0, 5), Block::Stone);
        world.save_chunks(&directory).unwrap();

        assert!(directory.join("chunk_0_0_0.vchk").exists());
        assert!(directory.join("chunk_1_0_0.vchk").exists());
        assert!(directory.join("chunk_1_0_0.vchk").exists());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 2);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn an_emptied_chunk_round_trips_as_an_empty_v3_file() {
        let directory = temp_dir("emptied");
        let mut world = World::default();
        let coordinate = IVec3::new(5, 0, 5);
        world.set_block(coordinate, Block::Stone);
        world.save_chunks(&directory).unwrap();

        world.set_block(coordinate, Block::Air);
        world.save_chunks(&directory).unwrap();

        let bytes = fs::read(directory.join("chunk_0_0_0.vchk")).unwrap();
        assert_eq!(u32::from_le_bytes(bytes[18..22].try_into().unwrap()), 0);

        let loaded = World::load_chunks(&directory).unwrap();
        assert_eq!(loaded.block(coordinate), Block::Air);
        assert!(loaded.iter_chunks().next().is_none());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn save_chunk_writes_a_modified_chunk_once() {
        let directory = temp_dir("save-one");
        let chunk = chunk_at(IVec3::new(5, 0, 5));
        let mut world = World::default();
        world.set_block(IVec3::new(5, 0, 5), Block::Stone);
        assert!(world.is_modified(chunk));

        world.save_chunk(&directory, chunk).unwrap();

        assert!(!world.is_modified(chunk));
        let saved = World::load_saved_chunk(&directory, chunk).unwrap().unwrap();
        assert_eq!(saved.blocks.get(&IVec3::new(5, 0, 5)), Some(&Block::Stone));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn removing_a_chunk_drops_blocks_but_keeps_the_marker() {
        let chunk = chunk_at(IVec3::new(5, 0, 5));
        let mut world = World::default();
        world.set_block(IVec3::new(5, 0, 5), Block::Stone);

        world.remove_chunk(chunk);

        assert!(world.chunk(chunk).is_none());
        assert_eq!(world.block(IVec3::new(5, 0, 5)), Block::Air);
        assert!(world.is_modified(chunk));
    }

    #[test]
    fn seed_file_is_created_once_and_reused() {
        let directory = temp_dir("seed-create");

        assert_eq!(load_or_create_seed(&directory, Some(42)).unwrap(), 42);
        assert_eq!(fs::read_to_string(directory.join("seed")).unwrap(), "42\n");
        assert_eq!(load_or_create_seed(&directory, Some(7)).unwrap(), 42);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_fresh_seed_is_written_as_decimal() {
        let directory = temp_dir("seed-fresh");

        let seed = load_or_create_seed(&directory, None).unwrap();

        let text = fs::read_to_string(directory.join("seed")).unwrap();
        assert_eq!(text, format!("{seed}\n"));
        assert_eq!(parse_seed(&text).unwrap(), seed);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn malformed_seed_files_are_rejected() {
        let directory = temp_dir("seed-malformed");
        let path = directory.join("seed");

        for text in [
            "",
            "abc",
            "-1",
            "1.5",
            "42\n\n",
            " 42",
            "18446744073709551616",
        ] {
            fs::write(&path, text).unwrap();
            assert!(
                matches!(
                    load_or_create_seed(&directory, None),
                    Err(SaveError::InvalidSeed(_))
                ),
                "{text:?}"
            );
        }

        fs::write(&path, "42\r\n").unwrap();
        assert_eq!(load_or_create_seed(&directory, None).unwrap(), 42);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_missing_seed_with_chunks_is_rejected() {
        let directory = temp_dir("seed-missing");
        let chunks = directory.join("chunks");
        fs::create_dir_all(&chunks).unwrap();
        write_raw(
            &chunks,
            "chunk_0_0_0.vchk",
            &header(chunk_at(IVec3::ZERO), 0),
        );

        assert!(matches!(
            load_or_create_seed(&directory, None),
            Err(SaveError::MissingSeed)
        ));
        assert!(!directory.join("seed").exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn bad_magic_is_rejected() {
        let directory = temp_dir("bad-magic");
        let mut bytes = header(ChunkCoord(IVec3::ZERO), 1);
        bytes[0] = b'X';
        bytes.extend_from_slice(&[0, 0, 0, 1]);
        write_raw(&directory, "chunk_0_0_0.vchk", &bytes);

        assert!(matches!(
            World::load_chunks(&directory),
            Err(SaveError::InvalidMagic)
        ));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let directory = temp_dir("version");
        let mut bytes = header(ChunkCoord(IVec3::ZERO), 1);
        bytes[4..6].copy_from_slice(&4_u16.to_le_bytes());
        bytes.extend_from_slice(&[0, 0, 0, 1]);
        write_raw(&directory, "chunk_0_0_0.vchk", &bytes);

        assert!(matches!(
            World::load_chunks(&directory),
            Err(SaveError::UnsupportedVersion(4))
        ));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn invalid_block_count_is_rejected() {
        let directory = temp_dir("count");
        write_raw(
            &directory,
            "chunk_0_0_0.vchk",
            &header(ChunkCoord(IVec3::ZERO), MAX_BLOCK_COUNT + 1),
        );
        assert!(matches!(
            World::load_chunks(&directory),
            Err(SaveError::InvalidBlockCount(value)) if value == MAX_BLOCK_COUNT + 1
        ));
        fs::remove_dir_all(directory).unwrap();

        let directory = temp_dir("count-v1");
        let mut bytes = header(ChunkCoord(IVec3::ZERO), 0);
        bytes[4..6].copy_from_slice(&1_u16.to_le_bytes());
        write_raw(&directory, "chunk_0_0_0.vchk", &bytes);
        assert!(matches!(
            World::load_chunks(&directory),
            Err(SaveError::InvalidBlockCount(0))
        ));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn invalid_local_coordinate_is_rejected() {
        let directory = temp_dir("local");
        let mut bytes = header(ChunkCoord(IVec3::ZERO), 1);
        bytes.extend_from_slice(&[32, 0, 0, 1]);
        write_raw(&directory, "chunk_0_0_0.vchk", &bytes);

        assert!(matches!(
            World::load_chunks(&directory),
            Err(SaveError::InvalidLocalCoordinate(IVec3 { x: 32, .. }))
        ));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn invalid_or_air_block_ids_are_rejected() {
        for block_id in [0, 9] {
            let directory = temp_dir("block-id");
            let mut bytes = header(ChunkCoord(IVec3::ZERO), 1);
            bytes.extend_from_slice(&[0, 0, 0, block_id]);
            write_raw(&directory, "chunk_0_0_0.vchk", &bytes);

            assert!(matches!(
                World::load_chunks(&directory),
                Err(SaveError::InvalidBlockId(value)) if value == block_id
            ));
            fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn version_3_round_trips_sand() {
        let directory = temp_dir("v3-sand");
        let mut world = World::default();
        world.set_block(IVec3::new(1, 0, 0), Block::Sand);
        world.save_chunks(&directory).unwrap();

        let loaded = World::load_chunks(&directory).unwrap();

        assert_eq!(loaded.block(IVec3::new(1, 0, 0)), Block::Sand);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn version_2_rejects_sand() {
        let directory = temp_dir("v2-sand");
        let mut bytes = header(ChunkCoord(IVec3::ZERO), 1);
        bytes[4..6].copy_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&[0, 0, 0, 8]);
        write_raw(&directory, "chunk_0_0_0.vchk", &bytes);

        assert!(matches!(
            World::load_chunks(&directory),
            Err(SaveError::InvalidBlockId(8))
        ));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn version_3_accepts_an_empty_modified_chunk_file() {
        let directory = temp_dir("v3-empty");
        write_raw(
            &directory,
            "chunk_0_0_0.vchk",
            &header(ChunkCoord(IVec3::ZERO), 0),
        );

        let world = World::load_chunks(&directory).unwrap();

        assert_eq!(
            world
                .chunk(chunk_at(IVec3::ZERO))
                .map_or(0, |chunk| chunk.blocks.len()),
            0
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn version_2_round_trips_lamp_and_water() {
        let directory = temp_dir("v2-round-trip");
        let mut world = World::default();
        world.set_block(IVec3::new(1, 0, 0), Block::Lamp);
        world.set_block(IVec3::new(2, 0, 0), Block::Water);
        world.save_chunks(&directory).unwrap();

        let loaded = World::load_chunks(&directory).unwrap();

        assert_eq!(loaded.block(IVec3::new(1, 0, 0)), Block::Lamp);
        assert_eq!(loaded.block(IVec3::new(2, 0, 0)), Block::Water);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn version_1_rejects_the_new_block_ids() {
        for block_id in [6, 7] {
            let directory = temp_dir("v1-block-id");
            let mut bytes = header(ChunkCoord(IVec3::ZERO), 1);
            bytes[4..6].copy_from_slice(&1_u16.to_le_bytes());
            bytes.extend_from_slice(&[0, 0, 0, block_id]);
            write_raw(&directory, "chunk_0_0_0.vchk", &bytes);

            assert!(matches!(
                World::load_chunks(&directory),
                Err(SaveError::InvalidBlockId(value)) if value == block_id
            ));
            fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn version_1_loads_the_old_block_ids() {
        let directory = temp_dir("v1-load");
        let mut bytes = header(ChunkCoord(IVec3::ZERO), 2);
        bytes[4..6].copy_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&[0, 0, 0, 1, 1, 0, 0, 5]);
        write_raw(&directory, "chunk_0_0_0.vchk", &bytes);

        let world = World::load_chunks(&directory).unwrap();

        assert_eq!(world.block(IVec3::new(0, 0, 0)), Block::Stone);
        assert_eq!(world.block(IVec3::new(1, 0, 0)), Block::Leaves);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn loading_recomputes_light_and_never_reads_it_from_disk() {
        let directory = temp_dir("relight");
        let mut world = World::default();
        for x in 0..4 {
            for z in 0..4 {
                world.set_block(IVec3::new(x, 0, z), Block::Dirt);
            }
        }
        world.set_block(IVec3::new(2, 1, 2), Block::Lamp);
        world.save_chunks(&directory).unwrap();

        let loaded = World::load_chunks(&directory).unwrap();
        let chunk = loaded.chunk(chunk_at(IVec3::ZERO)).unwrap();

        assert_eq!(chunk.light(IVec3::new(2, 1, 2)), (0, 14));
        assert_eq!(chunk.light(IVec3::new(2, 3, 2)).0, 15);
        assert_eq!(chunk.sky_light.iter().copied().max(), Some(15));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn duplicate_records_are_rejected() {
        let directory = temp_dir("duplicate");
        let mut bytes = header(ChunkCoord(IVec3::ZERO), 2);
        bytes.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 2]);
        write_raw(&directory, "chunk_0_0_0.vchk", &bytes);

        assert!(matches!(
            World::load_chunks(&directory),
            Err(SaveError::DuplicateRecord(IVec3 { x: 0, y: 0, z: 0 }))
        ));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn truncated_and_trailing_files_are_rejected() {
        let directory = temp_dir("length");
        write_raw(
            &directory,
            "chunk_0_0_0.vchk",
            &header(ChunkCoord(IVec3::ZERO), 1),
        );
        assert!(matches!(
            World::load_chunks(&directory),
            Err(SaveError::Truncated)
        ));

        let mut bytes = header(ChunkCoord(IVec3::ZERO), 1);
        bytes.extend_from_slice(&[0, 0, 0, 1, 0]);
        write_raw(&directory, "chunk_0_0_0.vchk", &bytes);
        assert!(matches!(
            World::load_chunks(&directory),
            Err(SaveError::TrailingBytes)
        ));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn file_name_and_header_must_match() {
        let directory = temp_dir("mismatch");
        let mut bytes = header(ChunkCoord(IVec3::new(1, 0, 0)), 1);
        bytes.extend_from_slice(&[0, 0, 0, 1]);
        write_raw(&directory, "chunk_0_0_0.vchk", &bytes);

        assert!(matches!(
            World::load_chunks(&directory),
            Err(SaveError::ChunkMismatch { .. })
        ));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn invalid_file_name_is_rejected() {
        let directory = temp_dir("name");
        write_raw(&directory, "chunk_bad.vchk", &[]);

        assert!(matches!(
            World::load_chunks(&directory),
            Err(SaveError::InvalidFileName(name)) if name == "chunk_bad.vchk"
        ));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn chunk_coordinates_must_fit_in_i32() {
        let directory = temp_dir("coordinate");
        let chunk = ChunkCoord(IVec3::new(67_108_864, 0, 0));
        let mut bytes = header(chunk, 1);
        bytes.extend_from_slice(&[0, 0, 0, 1]);
        write_raw(&directory, "chunk_67108864_0_0.vchk", &bytes);

        assert!(matches!(
            World::load_chunks(&directory),
            Err(SaveError::InvalidChunkCoordinate(IVec3 {
                x: 67_108_864,
                ..
            }))
        ));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn duplicate_header_chunks_are_rejected() {
        let directory = temp_dir("duplicate-chunk");
        let mut first = header(ChunkCoord(IVec3::ZERO), 1);
        first.extend_from_slice(&[0, 0, 0, 1]);
        write_raw(&directory, "chunk_0_0_0.vchk", &first);
        let mut second = header(ChunkCoord(IVec3::ZERO), 1);
        second.extend_from_slice(&[1, 0, 0, 1]);
        write_raw(&directory, "chunk_1_0_0.vchk", &second);

        assert!(matches!(
            World::load_chunks(&directory),
            Err(SaveError::DuplicateChunk(_))
        ));
        fs::remove_dir_all(directory).unwrap();
    }
}
