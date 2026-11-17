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

use glam::{IVec3, Vec3};
use thiserror::Error;

use crate::block::Block;

/// Edge length of a chunk on every axis.
pub(crate) const CHUNK_SIZE: i32 = 32;

/// Chunks touched by an edit, in deterministic order.
pub(crate) type DirtyChunks = BTreeSet<ChunkCoord>;

const MAGIC: [u8; 4] = *b"VCHK";
const FORMAT_VERSION: u16 = 1;
const HEADER_BYTES: usize = 22;
const RECORD_BYTES: usize = 4;
const MAX_BLOCK_COUNT: u32 = 32_768;
const CHUNK_FILE_PREFIX: &str = "chunk_";
const CHUNK_FILE_SUFFIX: &str = ".vchk";

/// Coordinate of one chunk in chunk space.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ChunkCoord(IVec3);

impl ChunkCoord {
    /// Splits a world coordinate into its chunk and local coordinate.
    pub(crate) fn from_world(world: IVec3) -> (Self, IVec3) {
        let size = IVec3::splat(CHUNK_SIZE);
        (Self(world.div_euclid(size)), world.rem_euclid(size))
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
#[derive(Debug, Default)]
pub(crate) struct Chunk {
    pub(crate) blocks: HashMap<IVec3, Block>,
}

impl Chunk {
    fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

/// Chunked block world.
///
/// Missing chunks are air, and every stored chunk is non-empty.
#[derive(Debug, Default)]
pub(crate) struct World {
    chunks: HashMap<ChunkCoord, Chunk>,
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

        if block == Block::Air {
            let Some(chunk_data) = self.chunks.get_mut(&chunk) else {
                return DirtyChunks::new();
            };
            chunk_data.blocks.remove(&local);
            if chunk_data.is_empty() {
                self.chunks.remove(&chunk);
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

    /// Iterates over every non-empty chunk in unspecified order.
    pub(crate) fn iter_chunks(&self) -> impl Iterator<Item = (ChunkCoord, &Chunk)> {
        self.chunks
            .iter()
            .map(|(coordinate, chunk)| (*coordinate, chunk))
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
            world.chunks.insert(header_chunk, chunk);
        }
        Ok(world)
    }

    /// Writes every non-empty chunk and removes stale chunk files.
    pub(crate) fn save_chunks(&self, directory: &Path) -> Result<(), SaveError> {
        fs::create_dir_all(directory).map_err(|error| SaveError::from_path(&error, directory))?;

        for entry in
            fs::read_dir(directory).map_err(|error| SaveError::from_path(&error, directory))?
        {
            let entry = entry.map_err(|error| SaveError::from_path(&error, directory))?;
            let path = entry.path();
            if !path.is_file() || path.extension().is_none_or(|extension| extension != "vchk") {
                continue;
            }
            let Some(file_name) = path.file_name().map(|name| name.to_string_lossy()) else {
                continue;
            };
            let Some(coord) = parse_chunk_file_name(&file_name) else {
                continue;
            };
            if !self.chunks.contains_key(&coord) {
                fs::remove_file(&path).map_err(|error| SaveError::from_path(&error, &path))?;
            }
        }

        let mut chunks: Vec<_> = self.chunks.iter().collect();
        chunks.sort_by_key(|(coord, _)| **coord);
        for (coord, chunk) in chunks {
            let bytes = serialize_chunk(*coord, chunk);
            let path = chunk_file_path(directory, *coord);
            fs::write(&path, bytes).map_err(|error| SaveError::from_path(&error, &path))?;
        }
        Ok(())
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
    if version != FORMAT_VERSION {
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
    if block_count == 0 || block_count > MAX_BLOCK_COUNT {
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
        let block = Block::from_id(block_id)
            .filter(|block| *block != Block::Air)
            .ok_or(SaveError::InvalidBlockId(block_id))?;
        if blocks.insert(local, block).is_some() {
            return Err(SaveError::DuplicateRecord(local));
        }
    }

    Ok((header_chunk, Chunk { blocks }))
}

fn serialize_chunk(chunk_coord: ChunkCoord, chunk: &Chunk) -> Vec<u8> {
    let mut records: Vec<_> = chunk.blocks.iter().collect();
    records.sort_by_key(|(local, _)| (local.x, local.y, local.z));

    let block_count = u32::try_from(records.len()).expect("chunk block count fits in u32");
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
    for (local, block) in records {
        bytes.push(u8::try_from(local.x).expect("local coordinate fits in u8"));
        bytes.push(u8::try_from(local.y).expect("local coordinate fits in u8"));
        bytes.push(u8::try_from(local.z).expect("local coordinate fits in u8"));
        bytes.push(block.id());
    }
    bytes
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
    fn saving_removes_stale_chunk_files() {
        let directory = temp_dir("stale");
        let mut world = World::default();
        world.set_block(IVec3::new(5, 0, 5), Block::Stone);
        world.save_chunks(&directory).unwrap();
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);

        World::default().save_chunks(&directory).unwrap();

        assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
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
        bytes[4..6].copy_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&[0, 0, 0, 1]);
        write_raw(&directory, "chunk_0_0_0.vchk", &bytes);

        assert!(matches!(
            World::load_chunks(&directory),
            Err(SaveError::UnsupportedVersion(2))
        ));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn invalid_block_count_is_rejected() {
        for count in [0, MAX_BLOCK_COUNT + 1] {
            let directory = temp_dir("count");
            write_raw(
                &directory,
                "chunk_0_0_0.vchk",
                &header(ChunkCoord(IVec3::ZERO), count),
            );

            assert!(matches!(
                World::load_chunks(&directory),
                Err(SaveError::InvalidBlockCount(value)) if value == count
            ));
            fs::remove_dir_all(directory).unwrap();
        }
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
        for block_id in [0, 6] {
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
