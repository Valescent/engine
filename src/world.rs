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

use glam::{IVec2, IVec3, Vec3};
use thiserror::Error;

use crate::block::Block;
use crate::light::{self, LightField};
use crate::section::{SectionEdits, SectionKey};

/// Edge length of a chunk on every axis.
pub(crate) const CHUNK_SIZE: i32 = 32;

/// Cells in one chunk.
pub(crate) const CHUNK_CELLS: usize = (CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE) as usize;

/// Bytes of packed block storage in one chunk: four bits per cell.
pub(crate) const CHUNK_STORAGE_BYTES: usize = CHUNK_CELLS / 2;

/// Chunks in one vertical world column.
pub(crate) const CHUNK_COLUMN: i32 = 4;

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
/// A stored chunk always holds at least one non-air block. Light is derived
/// state and lives in [`World`] as a separate field.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Chunk {
    cells: Arc<Vec<u8>>,
    filled: usize,
    water: usize,
    /// Highest solid block's local y, or `-1` when the chunk has none.
    top: i32,
    /// Highest water block's local y, or `-1` when the chunk has none.
    water_top: i32,
    /// Highest non-tree solid block's local y, or `-1` when the chunk has none.
    terrain_top: i32,
    /// Lowest terrain column top, or `-1` when any column has no terrain.
    ///
    /// Water is deliberately excluded: [`crate::horizon::Horizon`] treats this
    /// as a conservative occluder, and water-only ocean columns must not hide
    /// land behind them. Visible-water bounds come from `water_top` instead.
    terrain_min: i32,
}

impl Default for Chunk {
    fn default() -> Self {
        Self {
            cells: Arc::new(vec![0; CHUNK_STORAGE_BYTES]),
            filled: 0,
            water: 0,
            top: -1,
            water_top: -1,
            terrain_top: -1,
            terrain_min: i32::MAX,
        }
    }
}

impl Chunk {
    pub(crate) fn is_empty(&self) -> bool {
        self.filled == 0
    }

    /// Number of non-air blocks.
    pub(crate) fn len(&self) -> usize {
        self.filled
    }

    /// Whether the chunk holds any water, so a water mesh is worth building.
    pub(crate) fn has_water(&self) -> bool {
        self.water > 0
    }

    /// Highest solid block's local y, or `-1` when the chunk has none.
    ///
    /// Unlike [`Self::terrain_top`] this includes tree blocks, so the horizon
    /// candidate keeps a chunk visible while its canopy rises above the
    /// horizon. The occluder height still comes from terrain only.
    pub(crate) fn top(&self) -> i32 {
        self.top
    }

    /// Highest water block's local y, or `-1` when the chunk has none.
    pub(crate) fn water_top(&self) -> i32 {
        self.water_top
    }

    /// Highest non-tree solid block's local y, or `-1` when the chunk has none.
    #[cfg(test)]
    pub(crate) fn terrain_top(&self) -> i32 {
        self.terrain_top
    }

    /// Lowest terrain column top, or `-1` when any column has no terrain.
    pub(crate) fn terrain_min(&self) -> i32 {
        if self.terrain_min == i32::MIN || self.terrain_min == i32::MAX {
            -1
        } else {
            self.terrain_min
        }
    }

    /// Whether the cached minimum needs a recompute after an edit.
    pub(crate) fn terrain_min_stale(&self) -> bool {
        self.terrain_min == i32::MIN || self.terrain_min == i32::MAX
    }

    /// Recomputes the lowest terrain column top after a bulk fill or an edit.
    ///
    /// Every column contributes its highest terrain cell. Water alone does not
    /// count, so a water-only column makes the chunk unsuitable as a horizon
    /// occluder rather than contributing the water surface.
    pub(crate) fn recompute_terrain_min(&mut self) {
        let mut minimum = i32::MAX;
        for x in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                let mut top = -1;
                for y in (0..CHUNK_SIZE).rev() {
                    let index = local_index(IVec3::new(x, y, z)).expect("in-chunk cell");
                    let block = packed_block(&self.cells, index);
                    if block.is_terrain() {
                        top = y;
                        break;
                    }
                }
                if top < 0 {
                    self.terrain_min = -1;
                    return;
                }
                minimum = minimum.min(top);
            }
        }
        self.terrain_min = if minimum == i32::MAX { -1 } else { minimum };
    }

    /// Stores a terrain-only minimum computed by the generator's column pass.
    pub(crate) fn set_terrain_min(&mut self, minimum: i32) {
        self.terrain_min = if minimum < 0 { -1 } else { minimum };
    }

    /// Returns the block at a local coordinate, or `Air` outside the chunk.
    pub(crate) fn block(&self, local: IVec3) -> Block {
        local_index(local).map_or(Block::Air, |index| packed_block(&self.cells, index))
    }

    /// Stores a block at a local coordinate. Coordinates outside the chunk are ignored.
    pub(crate) fn set_block(&mut self, local: IVec3, block: Block) {
        let Some(index) = local_index(local) else {
            return;
        };
        let previous = packed_block(&self.cells, index);
        if previous == block {
            return;
        }
        match (previous, block) {
            (Block::Air, _) => self.filled += 1,
            (_, Block::Air) => self.filled -= 1,
            _ => {}
        }
        match (previous, block) {
            (Block::Water, _) => self.water -= 1,
            (_, Block::Water) => self.water += 1,
            _ => {}
        }
        let packed = block.id();
        let cells = Arc::make_mut(&mut self.cells);
        let byte = &mut cells[index / 2];
        if index.is_multiple_of(2) {
            *byte = (*byte & 0xF0) | (packed & 0x0F);
        } else {
            *byte = (*byte & 0x0F) | ((packed & 0x0F) << 4);
        }

        if block.is_solid() && local.y > self.top {
            self.top = local.y;
        } else if previous.is_solid() && !block.is_solid() && local.y == self.top {
            self.recompute_top();
        }

        if block == Block::Water && local.y > self.water_top {
            self.water_top = local.y;
        } else if previous == Block::Water && block != Block::Water && local.y == self.water_top {
            self.recompute_water_top();
        }

        if block.is_terrain() && local.y > self.terrain_top {
            self.terrain_top = local.y;
        } else if previous.is_terrain() && !block.is_terrain() && local.y == self.terrain_top {
            self.recompute_terrain_top();
        }

        if block.is_terrain()
            || block == Block::Water
            || previous.is_terrain()
            || previous == Block::Water
        {
            self.terrain_min = i32::MIN;
        }
    }

    /// Fills a vertical run of air cells in one column with `block`.
    ///
    /// This is the bulk generation path for section samples: it writes the
    /// packed nibbles in one pass and updates the height and water counters
    /// once per run instead of once per cell. The run must currently be air;
    /// the debug build asserts that contract. Coordinates outside the chunk are
    /// clamped.
    pub(crate) fn fill_column(&mut self, x: i32, z: i32, y_start: i32, y_end: i32, block: Block) {
        debug_assert_ne!(block, Block::Air, "fill_column writes non-air blocks");
        let start = y_start.max(0);
        let end = y_end.min(CHUNK_SIZE - 1);
        if start > end || !(0..CHUNK_SIZE).contains(&x) || !(0..CHUNK_SIZE).contains(&z) {
            return;
        }
        let count = usize::try_from(end - start + 1).expect("run length fits");
        self.filled += count;
        if block == Block::Water {
            self.water += count;
            self.water_top = self.water_top.max(end);
        } else if block.is_solid() {
            self.top = self.top.max(end);
            if block.is_terrain() {
                self.terrain_top = self.terrain_top.max(end);
            }
        }
        if block.is_terrain() || block == Block::Water {
            self.terrain_min = i32::MIN;
        }

        let stride = usize::try_from(CHUNK_SIZE).expect("chunk size fits");
        let base = usize::try_from(x * CHUNK_SIZE * CHUNK_SIZE + z).expect("column base fits");
        let packed = block.id();
        let cells = Arc::make_mut(&mut self.cells);
        let mut index = base + usize::try_from(start).expect("local y fits") * stride;
        for _ in 0..count {
            debug_assert_eq!(
                packed_block(cells, index),
                Block::Air,
                "fill_column must only write air cells"
            );
            let byte = &mut cells[index / 2];
            if index.is_multiple_of(2) {
                *byte = (*byte & 0xF0) | (packed & 0x0F);
            } else {
                *byte = (*byte & 0x0F) | ((packed & 0x0F) << 4);
            }
            index += stride;
        }
    }

    /// Recomputes the highest solid cell after the top block is cleared.
    fn recompute_top(&mut self) {
        for y in (0..CHUNK_SIZE).rev() {
            for x in 0..CHUNK_SIZE {
                for z in 0..CHUNK_SIZE {
                    let index = local_index(IVec3::new(x, y, z)).expect("in-chunk cell");
                    if packed_block(&self.cells, index).is_solid() {
                        self.top = y;
                        return;
                    }
                }
            }
        }
        self.top = -1;
    }

    /// Recomputes the highest water cell after the top water is cleared.
    fn recompute_water_top(&mut self) {
        for y in (0..CHUNK_SIZE).rev() {
            for x in 0..CHUNK_SIZE {
                for z in 0..CHUNK_SIZE {
                    let index = local_index(IVec3::new(x, y, z)).expect("in-chunk cell");
                    if packed_block(&self.cells, index) == Block::Water {
                        self.water_top = y;
                        return;
                    }
                }
            }
        }
        self.water_top = -1;
    }

    /// Recomputes the highest non-tree solid cell after the top one is cleared.
    fn recompute_terrain_top(&mut self) {
        for y in (0..CHUNK_SIZE).rev() {
            for x in 0..CHUNK_SIZE {
                for z in 0..CHUNK_SIZE {
                    let index = local_index(IVec3::new(x, y, z)).expect("in-chunk cell");
                    if packed_block(&self.cells, index).is_terrain() {
                        self.terrain_top = y;
                        return;
                    }
                }
            }
        }
        self.terrain_top = -1;
    }

    /// Shared packed storage, cloned for a worker snapshot.
    pub(crate) fn shared_storage(&self) -> Arc<Vec<u8>> {
        Arc::clone(&self.cells)
    }

    /// Non-air blocks in local `(x, y, z)` order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (IVec3, Block)> + '_ {
        (0..CHUNK_CELLS).filter_map(move |index| {
            let block = packed_block(&self.cells, index);
            (block != Block::Air).then(|| (local_from_index(index), block))
        })
    }
}

/// Decodes one packed cell, low nibble first within each byte.
pub(crate) fn packed_block(cells: &[u8], index: usize) -> Block {
    let byte = cells[index / 2];
    let id = if index.is_multiple_of(2) {
        byte & 0x0F
    } else {
        byte >> 4
    };
    Block::from_id(id).unwrap_or(Block::Air)
}

/// Linear index of a chunk-local coordinate, or `None` outside the chunk.
fn local_index(local: IVec3) -> Option<usize> {
    if !(local.cmpge(IVec3::ZERO).all() && local.cmplt(IVec3::splat(CHUNK_SIZE)).all()) {
        return None;
    }
    let stride = usize::try_from(CHUNK_SIZE).expect("chunk size fits");
    let x = usize::try_from(local.x).expect("local coordinate fits");
    let y = usize::try_from(local.y).expect("local coordinate fits");
    let z = usize::try_from(local.z).expect("local coordinate fits");
    Some((x * stride + y) * stride + z)
}

/// Chunk-local coordinate of a linear block index.
fn local_from_index(index: usize) -> IVec3 {
    let stride = usize::try_from(CHUNK_SIZE).expect("chunk size fits");
    let x = index / (stride * stride);
    let y = (index / stride) % stride;
    let z = index % stride;
    IVec3::new(
        i32::try_from(x).expect("local x fits"),
        i32::try_from(y).expect("local y fits"),
        i32::try_from(z).expect("local z fits"),
    )
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
            .map_or(Block::Air, |chunk_data| chunk_data.block(local))
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

        let Some(chunk_data) = self.chunks.get_mut(&chunk) else {
            let mut chunk_data = Chunk::default();
            chunk_data.set_block(local, block);
            self.chunks.insert(chunk, chunk_data);
            return dirty;
        };
        chunk_data.set_block(local, block);
        if chunk_data.is_empty() {
            self.chunks.remove(&chunk);
            self.light.remove(&chunk);
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

    /// Inserts a generated or loaded chunk. Empty chunks are ignored.
    pub(crate) fn insert_chunk(&mut self, chunk: ChunkCoord, data: Chunk) {
        if data.is_empty() {
            return;
        }
        self.chunks.insert(chunk, data);
    }

    /// Drops one chunk from memory, keeping its modified marker.
    pub(crate) fn remove_chunk(&mut self, chunk: ChunkCoord) {
        self.chunks.remove(&chunk);
        self.light.remove(&chunk);
    }

    /// Bytes held by stored blocks and light fields, excluding meshes and GPU state.
    pub(crate) fn resident_bytes(&self) -> usize {
        self.chunks.len() * CHUNK_STORAGE_BYTES
            + self
                .light
                .values()
                .map(|field| field.resident_bytes())
                .sum::<usize>()
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

    /// Every stored chunk in the eight columns around `column`.
    ///
    /// Column loads and unloads change the sky view of their neighbours, so
    /// those neighbours need a fresh light field even though their own blocks
    /// did not change.
    pub(crate) fn neighbour_column_chunks(&self, column: IVec2) -> DirtyChunks {
        let mut chunks = DirtyChunks::new();
        for dx in -1..=1 {
            for dz in -1..=1 {
                if dx == 0 && dz == 0 {
                    continue;
                }
                for y in 0..CHUNK_COLUMN {
                    let chunk = ChunkCoord::new(column.x + dx, y, column.y + dz);
                    if self.chunks.contains_key(&chunk) {
                        chunks.insert(chunk);
                    }
                }
            }
        }
        chunks
    }

    /// Stores an accepted light field for a chunk.
    pub(crate) fn set_light_field(&mut self, chunk: ChunkCoord, field: LightField) {
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
    /// Recomputes cached horizon minima for chunks edited since the last build.
    pub(crate) fn refresh_terrain_min(&mut self) {
        for chunk in self.chunks.values_mut() {
            if chunk.terrain_min_stale() {
                chunk.recompute_terrain_min();
            }
        }
    }

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
            self.light.insert(chunk, field);
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
            if !chunk.is_empty() {
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
    ///
    /// Returns whether a file was written.
    pub(crate) fn save_chunk(
        &mut self,
        directory: &Path,
        chunk: ChunkCoord,
    ) -> Result<bool, SaveError> {
        if !self.modified.contains(&chunk) {
            return Ok(false);
        }
        fs::create_dir_all(directory).map_err(|error| SaveError::from_path(&error, directory))?;
        let bytes = match self.chunks.get(&chunk) {
            Some(data) => serialize_chunk(chunk, data),
            None => serialize_empty_chunk(chunk),
        };
        let path = chunk_file_path(directory, chunk);
        fs::write(&path, bytes).map_err(|error| SaveError::from_path(&error, &path))?;
        self.modified.remove(&chunk);
        Ok(true)
    }

    /// Coordinates with saved chunk files in `directory`.
    ///
    /// Listing the directory once replaces a failed `fs::read` for every
    /// generated chunk. Files whose names do not parse, or whose coordinate
    /// would overflow a world origin, are ignored here; the per-chunk loader
    /// still reports them if they are requested.
    pub(crate) fn saved_chunk_index(directory: &Path) -> Result<HashSet<ChunkCoord>, SaveError> {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashSet::new()),
            Err(error) => return Err(SaveError::from_path(&error, directory)),
        };
        let mut chunks = HashSet::new();
        for entry in entries {
            let entry = entry.map_err(|error| SaveError::from_path(&error, directory))?;
            let path = entry.path();
            if !path.is_file() || path.extension().is_none_or(|extension| extension != "vchk") {
                continue;
            }
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            if let Some(chunk) = parse_chunk_file_name(&name)
                && chunk_origin_checked(chunk).is_some()
            {
                chunks.insert(chunk);
            }
        }
        Ok(chunks)
    }

    /// Coordinates with unsaved edits.
    pub(crate) fn modified_chunks(&self) -> impl Iterator<Item = ChunkCoord> + '_ {
        self.modified.iter().copied()
    }

    /// Coarse-section overlay for the modified chunks overlapping `key`.
    ///
    /// Only chunks with an edit marker are included: an unmodified resident
    /// chunk must stay seed-derived so caves and trees do not leak into the
    /// coarse mesh. A modified chunk that is no longer resident is read from
    /// `directory`. With no directory, or when the read fails, that chunk is
    /// omitted and the coarse section keeps the seed sample for that volume;
    /// this is conservative because a missing edit only leaves the far mesh
    /// stale, never wrong blocks.
    pub(crate) fn section_edits(
        &self,
        key: SectionKey,
        directory: Option<&Path>,
        saved: &HashSet<ChunkCoord>,
    ) -> SectionEdits {
        let mut edits = SectionEdits::new();
        let section_origin = key.origin();
        let section_max = section_origin + IVec3::splat(key.extent() - 1);
        for chunk in self.modified.union(saved) {
            // Saved-file coordinates are filtered by `saved_chunk_index`, but a
            // caller-supplied set can still hold a coordinate whose origin
            // overflows; skip it instead of panicking in `world_origin`.
            let Some(origin) = chunk_origin_checked(*chunk) else {
                continue;
            };
            let max = origin + IVec3::splat(CHUNK_SIZE - 1);
            if !origin.cmple(section_max).all() || !section_origin.cmple(max).all() {
                continue;
            }
            if let Some(data) = self.chunks.get(chunk) {
                edits.insert(*chunk, data.clone());
                continue;
            }
            let Some(directory) = directory else {
                continue;
            };
            match Self::load_saved_chunk(directory, *chunk) {
                Ok(Some(data)) => {
                    edits.insert(*chunk, data);
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(
                        %error,
                        ?chunk,
                        ?key,
                        "failed to load an edited chunk for a coarse section"
                    );
                }
            }
        }
        edits
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
    #[cfg(test)]
    pub(crate) fn mark_modified(&mut self, chunk: ChunkCoord) {
        self.modified.insert(chunk);
    }

    /// Clears every unsaved-edit marker after a successful save.
    pub(crate) fn clear_modified(&mut self) {
        self.modified.clear();
    }

    /// Whether a chunk has unsaved edits.
    #[cfg(test)]
    pub(crate) fn is_modified(&self, chunk: ChunkCoord) -> bool {
        self.modified.contains(&chunk)
    }

    /// Number of chunks with unsaved edits.
    #[cfg(test)]
    pub(crate) fn modified_count(&self) -> usize {
        self.modified.len()
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

    let mut chunk = Chunk::default();
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
        if chunk.block(local) != Block::Air {
            return Err(SaveError::DuplicateRecord(local));
        }
        chunk.set_block(local, block);
    }

    chunk.recompute_terrain_min();
    Ok((header_chunk, chunk))
}

fn serialize_chunk(chunk_coord: ChunkCoord, chunk: &Chunk) -> Vec<u8> {
    let block_count = u32::try_from(chunk.len()).expect("chunk block count fits in u32");
    let mut bytes = serialize_header(chunk_coord, block_count);
    for (local, block) in chunk.iter() {
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
    use crate::generation::{Generator, WORLD_HEIGHT};
    use crate::section::generate_section;

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

    fn sample_of(key: SectionKey, world: IVec3) -> IVec3 {
        (world - key.origin()).div_euclid(IVec3::splat(key.scale()))
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
    fn fill_column_matches_per_cell_writes() {
        let mut bulk = Chunk::default();
        let mut per_cell = Chunk::default();
        for (x, z, block) in [
            (0, 0, Block::Stone),
            (5, 9, Block::Water),
            (CHUNK_SIZE - 1, CHUNK_SIZE - 1, Block::Sand),
        ] {
            for y in 3..=17 {
                per_cell.set_block(IVec3::new(x, y, z), block);
            }
            bulk.fill_column(x, z, 3, 17, block);
        }

        assert_eq!(bulk, per_cell);
        assert_eq!(bulk.len(), 3 * 15);
        assert!(bulk.has_water());
        assert_eq!(bulk.water_top(), 17);
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
    fn neighbour_column_chunks_covers_only_stored_surrounding_columns() {
        let mut world = World::default();
        for (coordinate, block) in [
            (IVec3::new(1, 0, 1), Block::Stone),
            (IVec3::new(1, 64, 1), Block::Stone),
            (IVec3::new(64, 32, 64), Block::Stone),
            (IVec3::new(33, 96, 1), Block::Stone),
            // Excluded: the column itself, a non-adjacent column, and a chunk
            // above the loaded column height.
            (IVec3::new(33, 0, 33), Block::Stone),
            (IVec3::new(96, 0, 33), Block::Stone),
            (IVec3::new(1, 128, 1), Block::Stone),
        ] {
            world.set_block(coordinate, block);
        }

        let chunks = world.neighbour_column_chunks(IVec2::new(1, 1));
        let expected: DirtyChunks = [
            ChunkCoord::new(0, 0, 0),
            ChunkCoord::new(0, 2, 0),
            ChunkCoord::new(2, 1, 2),
            ChunkCoord::new(1, 3, 0),
        ]
        .into_iter()
        .collect();

        assert_eq!(chunks, expected);
        assert!(!chunks.contains(&ChunkCoord::new(0, 1, 0)));
        assert!(!chunks.contains(&ChunkCoord::new(1, 0, 1)));
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
    fn top_tracks_the_highest_solid_block() {
        let mut chunk = Chunk::default();
        assert_eq!(chunk.top(), -1);

        chunk.set_block(IVec3::new(1, 5, 1), Block::Stone);
        assert_eq!(chunk.top(), 5);

        chunk.set_block(IVec3::new(2, 9, 3), Block::Water);
        assert_eq!(chunk.top(), 5, "water does not occlude");

        chunk.set_block(IVec3::new(2, 9, 3), Block::Stone);
        assert_eq!(chunk.top(), 9);

        chunk.set_block(IVec3::new(5, 9, 5), Block::Stone);
        chunk.set_block(IVec3::new(2, 9, 3), Block::Air);
        assert_eq!(chunk.top(), 9, "another solid at the same height survives");

        chunk.set_block(IVec3::new(5, 9, 5), Block::Air);
        assert_eq!(chunk.top(), 5);

        chunk.set_block(IVec3::new(1, 5, 1), Block::Air);
        assert_eq!(chunk.top(), -1);
    }

    #[test]
    fn water_top_tracks_the_highest_water_block() {
        let mut chunk = Chunk::default();
        assert_eq!(chunk.water_top(), -1);

        chunk.set_block(IVec3::new(1, 3, 1), Block::Water);
        assert_eq!(chunk.water_top(), 3);

        chunk.set_block(IVec3::new(2, 7, 2), Block::Stone);
        assert_eq!(chunk.water_top(), 3, "solid blocks are not water");

        chunk.set_block(IVec3::new(2, 9, 2), Block::Water);
        assert_eq!(chunk.water_top(), 9);

        chunk.set_block(IVec3::new(3, 9, 3), Block::Water);
        chunk.set_block(IVec3::new(2, 9, 2), Block::Air);
        assert_eq!(
            chunk.water_top(),
            9,
            "another water at the same height survives"
        );

        chunk.set_block(IVec3::new(3, 9, 3), Block::Stone);
        assert_eq!(chunk.water_top(), 3, "replacing the top water recomputes");

        chunk.set_block(IVec3::new(1, 3, 1), Block::Air);
        assert_eq!(chunk.water_top(), -1);
    }

    #[test]
    fn terrain_top_ignores_trees_and_recomputes() {
        let mut chunk = Chunk::default();
        assert_eq!(chunk.terrain_top(), -1);

        chunk.set_block(IVec3::new(1, 5, 1), Block::Grass);
        chunk.set_block(IVec3::new(1, 6, 1), Block::Wood);
        chunk.set_block(IVec3::new(1, 7, 1), Block::Leaves);
        assert_eq!(chunk.terrain_top(), 5, "a tree is not a terrain wall");
        assert_eq!(chunk.top(), 7);

        chunk.set_block(IVec3::new(1, 6, 1), Block::Stone);
        assert_eq!(chunk.terrain_top(), 6);

        chunk.set_block(IVec3::new(1, 6, 1), Block::Wood);
        assert_eq!(chunk.terrain_top(), 5, "a trunk is not terrain");

        chunk.set_block(IVec3::new(1, 5, 1), Block::Air);
        assert_eq!(chunk.terrain_top(), -1);
    }

    #[test]
    fn terrain_min_ignores_water_only_columns() {
        let mut column = Chunk::default();
        column.set_block(IVec3::new(0, 5, 0), Block::Water);
        column.recompute_terrain_min();
        assert_eq!(
            column.terrain_min(),
            -1,
            "a water-only column is not an occluder"
        );

        let mut ocean = Chunk::default();
        for x in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                ocean.set_block(IVec3::new(x, 0, z), Block::Water);
            }
        }
        ocean.recompute_terrain_min();
        assert_eq!(
            ocean.terrain_min(),
            -1,
            "a water-only chunk is not an occluder"
        );
    }

    #[test]
    fn terrain_min_uses_the_lowest_terrain_top_with_water_present() {
        let mut chunk = Chunk::default();
        for x in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                if x != 0 || z != 0 {
                    chunk.set_block(IVec3::new(x, 9, z), Block::Stone);
                }
            }
        }
        chunk.set_block(IVec3::new(0, 5, 0), Block::Stone);
        // Water sits above the terrain in several columns, including the
        // lowest one, and must not raise the occluder.
        chunk.set_block(IVec3::new(0, 6, 0), Block::Water);
        chunk.set_block(IVec3::new(4, 12, 4), Block::Water);
        chunk.set_block(IVec3::new(8, 15, 8), Block::Water);

        chunk.recompute_terrain_min();
        assert_eq!(chunk.terrain_min(), 5);
        assert_eq!(chunk.water_top(), 15);
    }

    #[test]
    fn stored_chunks_never_contain_air() {
        let mut world = World::default();
        world.set_block(IVec3::new(5, 5, 5), Block::Stone);
        world.set_block(IVec3::new(6, 5, 5), Block::Dirt);
        world.set_block(IVec3::new(6, 5, 5), Block::Air);

        for (_, chunk) in world.iter_chunks() {
            assert!(chunk.iter().all(|(_, block)| block != Block::Air));
        }
    }

    #[test]
    fn chunk_storage_is_four_bits_per_cell() {
        let mut chunk = Chunk::default();
        assert_eq!(chunk.shared_storage().len(), CHUNK_STORAGE_BYTES);
        assert_eq!(CHUNK_STORAGE_BYTES, CHUNK_CELLS / 2);

        let blocks = [
            Block::Stone,
            Block::Dirt,
            Block::Grass,
            Block::Wood,
            Block::Leaves,
            Block::Lamp,
            Block::Water,
            Block::Sand,
        ];
        for (offset, block) in blocks.into_iter().enumerate() {
            let local = IVec3::new(i32::try_from(offset).unwrap(), 3, 5);
            chunk.set_block(local, block);
            assert_eq!(chunk.block(local), block);
        }
        assert_eq!(chunk.len(), blocks.len());

        let cleared = IVec3::new(1, 3, 5);
        chunk.set_block(cleared, Block::Air);
        assert_eq!(chunk.block(cleared), Block::Air);
        assert_eq!(chunk.len(), blocks.len() - 1);
        assert_eq!(
            chunk.iter().count(),
            blocks.len() - 1,
            "iteration skips the cleared cell"
        );
    }

    #[test]
    fn water_count_tracks_placements_replacements_and_removals() {
        let mut world = World::default();
        let chunk = chunk_at(IVec3::ZERO);
        let first = IVec3::new(3, 4, 5);
        let second = IVec3::new(4, 4, 5);

        world.set_block(first, Block::Water);
        world.set_block(first, Block::Water);
        let data = world.chunk(chunk).expect("stored chunk");
        assert!(data.has_water());
        assert_eq!(data.water, 1);

        world.set_block(first, Block::Stone);
        let data = world.chunk(chunk).expect("stored chunk");
        assert!(!data.has_water());
        assert_eq!(data.water, 0);

        world.set_block(first, Block::Water);
        world.set_block(second, Block::Water);
        let data = world.chunk(chunk).expect("stored chunk");
        assert!(data.has_water());
        assert_eq!(data.water, 2);
        assert_eq!(data.len(), 2);

        world.set_block(first, Block::Air);
        let data = world.chunk(chunk).expect("stored chunk");
        assert!(data.has_water());
        assert_eq!(data.water, 1);

        world.set_block(second, Block::Air);
        assert!(world.chunk(chunk).is_none());
    }

    #[test]
    fn chunk_lookup_matches_storage() {
        let mut world = World::default();
        world.set_block(IVec3::new(32, 0, 0), Block::Grass);

        let stored = world.chunk(chunk_at(IVec3::new(32, 0, 0))).unwrap();
        assert_eq!(stored.block(IVec3::ZERO), Block::Grass);
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
    fn clearing_the_modified_set_makes_the_next_save_write_nothing() {
        let directory = temp_dir("second-save");
        let second = temp_dir("second-save-empty");
        let mut world = World::default();
        world.set_block(IVec3::new(5, 0, 5), Block::Stone);
        world.save_chunks(&directory).unwrap();
        assert_eq!(world.modified_count(), 1);

        world.clear_modified();
        assert_eq!(world.modified_count(), 0);

        world.save_chunks(&second).unwrap();

        assert_eq!(fs::read_dir(&second).unwrap().count(), 0);
        fs::remove_dir_all(directory).unwrap();
        fs::remove_dir_all(second).unwrap();
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
    fn saved_chunk_index_lists_only_parseable_chunk_files() {
        let directory = temp_dir("saved-index");
        let mut world = World::default();
        world.set_block(IVec3::new(5, 0, 5), Block::Stone);
        world.save_chunks(&directory).unwrap();
        fs::write(directory.join("notes.txt"), []).unwrap();
        fs::write(directory.join("chunk_bad.vchk"), []).unwrap();

        let index = World::saved_chunk_index(&directory).unwrap();

        assert_eq!(index, HashSet::from([chunk_at(IVec3::new(5, 0, 5))]));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn saved_chunk_index_ignores_coordinates_that_overflow_the_origin() {
        let directory = temp_dir("saved-index-extreme");
        let mut world = World::default();
        world.set_block(IVec3::new(5, 0, 5), Block::Stone);
        world.save_chunks(&directory).unwrap();
        fs::write(directory.join("chunk_2147483647_0_0.vchk"), []).unwrap();
        fs::write(directory.join("chunk_0_-2147483648_0.vchk"), []).unwrap();

        let index = World::saved_chunk_index(&directory).unwrap();

        assert_eq!(index, HashSet::from([chunk_at(IVec3::new(5, 0, 5))]));
        for name in ["chunk_2147483647_0_0.vchk", "chunk_0_-2147483648_0.vchk"] {
            let chunk = parse_chunk_file_name(name).unwrap();
            assert!(
                chunk_origin_checked(chunk).is_none(),
                "{name} must be filtered before its world origin can overflow"
            );
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn saved_chunk_index_is_empty_for_a_missing_directory() {
        let directory = temp_dir("saved-index-missing").join("nested");

        assert!(World::saved_chunk_index(&directory).unwrap().is_empty());
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
        assert_eq!(saved.block(IVec3::new(5, 0, 5)), Block::Stone);
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
    fn section_edits_apply_a_modified_resident_chunk() {
        let generator = Generator::new(42);
        let world_position = IVec3::new(1, WORLD_HEIGHT, 1);
        let key = SectionKey::from_world(1, world_position);
        let mut world = World::default();
        world.set_block(world_position, Block::Stone);
        let chunk = chunk_at(world_position);

        let edits = world.section_edits(key, None, &HashSet::new());

        assert_eq!(edits.keys().copied().collect::<Vec<_>>(), vec![chunk]);
        let sample = sample_of(key, world_position);
        assert_eq!(
            generate_section(&generator, key, &edits).sample(sample),
            Block::Stone
        );
        assert_eq!(
            generate_section(&generator, key, &SectionEdits::new()).sample(sample),
            Block::Air,
            "the seed above the world is air"
        );
    }

    #[test]
    fn section_edits_load_a_modified_chunk_that_left_memory() {
        let directory = temp_dir("section-edits-saved");
        let generator = Generator::new(42);
        let world_position = IVec3::new(1, WORLD_HEIGHT, 1);
        let key = SectionKey::from_world(1, world_position);
        let mut world = World::default();
        world.set_block(world_position, Block::Stone);
        let chunk = chunk_at(world_position);
        world.save_chunks(&directory).unwrap();
        world.remove_chunk(chunk);
        assert!(world.chunk(chunk).is_none());
        assert!(world.is_modified(chunk));

        let edits = world.section_edits(key, Some(&directory), &HashSet::new());

        assert_eq!(edits.keys().copied().collect::<Vec<_>>(), vec![chunk]);
        assert_eq!(
            generate_section(&generator, key, &edits).sample(sample_of(key, world_position)),
            Block::Stone
        );
        assert!(
            world.section_edits(key, None, &HashSet::new()).is_empty(),
            "an unloaded edit has no overlay without a save directory"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn section_edits_include_only_modified_chunks_inside_the_section() {
        let key = SectionKey::new(1, IVec3::ZERO);
        let mut world = World::default();
        let mut resident = Chunk::default();
        resident.set_block(IVec3::ZERO, Block::Stone);
        world.insert_chunk(ChunkCoord::new(0, 0, 0), resident);
        assert!(world.section_edits(key, None, &HashSet::new()).is_empty());

        let inside = IVec3::new(1, 0, 1);
        let outside = IVec3::new(1, 0, 200);
        world.set_block(inside, Block::Stone);
        world.set_block(outside, Block::Stone);

        let edits = world.section_edits(key, None, &HashSet::new());
        assert_eq!(
            edits.keys().copied().collect::<Vec<_>>(),
            vec![chunk_at(inside)]
        );
    }

    #[test]
    fn section_edits_include_a_saved_resident_chunk_without_a_modified_marker() {
        let key = SectionKey::new(1, IVec3::ZERO);
        let chunk = ChunkCoord::new(0, 0, 0);
        let mut world = World::default();
        let mut data = Chunk::default();
        data.set_block(IVec3::ZERO, Block::Stone);
        world.insert_chunk(chunk, data);
        assert!(!world.is_modified(chunk));

        let edits = world.section_edits(key, None, &HashSet::from([chunk]));

        assert!(edits.contains_key(&chunk));
    }

    #[test]
    fn a_generated_chunk_survives_save_and_load_unchanged() {
        let directory = temp_dir("generated-round-trip");
        let chunk = chunk_at(IVec3::new(5, 40, 5));
        let generated = crate::generation::Generator::new(42).generate_chunk(chunk);
        let mut world = World::default();
        world.insert_chunk(chunk, generated.clone());
        world.mark_modified(chunk);
        world.save_chunks(&directory).unwrap();
        let path = directory.join("chunk_0_1_0.vchk");
        let first = fs::read(&path).unwrap();

        let saved = World::load_saved_chunk(&directory, chunk).unwrap().unwrap();
        assert_eq!(saved, generated);

        let mut reloaded = World::default();
        reloaded.insert_chunk(chunk, saved);
        reloaded.mark_modified(chunk);
        reloaded.save_chunks(&directory).unwrap();
        assert_eq!(fs::read(&path).unwrap(), first);
        fs::remove_dir_all(directory).unwrap();
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

        assert_eq!(world.chunk(chunk_at(IVec3::ZERO)).map_or(0, Chunk::len), 0);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn version_2_fixture_loads_and_resaves_as_v3() {
        let directory = temp_dir("v2-fixture");
        let chunk = chunk_at(IVec3::ZERO);
        let records = [
            (IVec3::new(0, 0, 0), Block::Stone),
            (IVec3::new(1, 0, 0), Block::Dirt),
            (IVec3::new(2, 0, 0), Block::Grass),
            (IVec3::new(3, 0, 0), Block::Wood),
            (IVec3::new(4, 0, 0), Block::Leaves),
            (IVec3::new(5, 0, 0), Block::Lamp),
            (IVec3::new(6, 0, 0), Block::Water),
        ];

        let mut bytes = header(chunk, u32::try_from(records.len()).unwrap());
        bytes[4..6].copy_from_slice(&2_u16.to_le_bytes());
        for (local, block) in records {
            bytes.extend_from_slice(&[
                u8::try_from(local.x).unwrap(),
                u8::try_from(local.y).unwrap(),
                u8::try_from(local.z).unwrap(),
                block.id(),
            ]);
        }

        let (header_chunk, parsed) = parse_chunk(&bytes).unwrap();
        assert_eq!(header_chunk, chunk);
        assert_eq!(parsed.len(), records.len());
        for (local, block) in records {
            assert_eq!(parsed.block(local), block, "{local}");
        }

        write_raw(&directory, "chunk_0_0_0.vchk", &bytes);
        let mut world = World::default();
        world.insert_chunk(chunk, parsed);
        world.mark_modified(chunk);
        world.save_chunks(&directory).unwrap();

        let saved = fs::read(directory.join("chunk_0_0_0.vchk")).unwrap();
        assert_eq!(
            u16::from_le_bytes(saved[4..6].try_into().unwrap()),
            FORMAT_VERSION
        );
        let loaded = World::load_chunks(&directory).unwrap();
        for (local, block) in records {
            assert_eq!(loaded.block(local), block, "{local}");
        }
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
        let field = loaded.light_field(chunk_at(IVec3::ZERO)).unwrap();

        assert_eq!(field.block(IVec3::new(2, 1, 2)), 14);
        assert_eq!(field.sky(IVec3::new(2, 3, 2)), 15);
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
