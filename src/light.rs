//! Voxel sky and block light: 4-bit storage, sources, and BFS propagation.
//!
//! Light is derived state. It is recomputed from blocks and never saved.
//! Propagation runs over a 3x3x3 chunk neighbourhood so values on the one-cell
//! border of a chunk are exact, then the centre chunk plus border is extracted
//! as a [`LightField`].

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use glam::IVec3;

use crate::block::Block;
use crate::mesh::ChunkSnapshot;
use crate::world::{CHUNK_COLUMN, CHUNK_SIZE, Chunk, ChunkCoord, World, packed_block};

/// Maximum light level.
pub(crate) const MAX_LIGHT: u8 = 15;

/// Block light emitted by a `Lamp`.
pub(crate) const LAMP_LIGHT: u8 = 14;

/// Cells per chunk axis.
const SIZE: i32 = CHUNK_SIZE;

/// Cells per axis of the 3x3x3 propagation grid.
const GRID: i32 = SIZE * 3;

/// Cells per axis of the stored field: the chunk plus one border cell.
const PADDED: i32 = CHUNK_SIZE + 2;

/// Cells per axis of the coarse BFS grid: the padded field plus a solid
/// sentinel layer on every side, so neighbour lookups need no bounds checks.
const COARSE_GRID: i32 = PADDED + 2;

const NEIGHBOURS: [IVec3; 6] = [
    IVec3::X,
    IVec3::NEG_X,
    IVec3::Y,
    IVec3::NEG_Y,
    IVec3::Z,
    IVec3::NEG_Z,
];

/// Immutable blocks a light job needs: the 3x3x3 neighbourhood plus every
/// loaded chunk above it in the same 3x3 footprint.
#[derive(Clone, Debug, Default)]
pub(crate) struct LightSnapshot {
    cells: HashMap<ChunkCoord, ChunkSnapshot>,
}

impl LightSnapshot {
    /// Copies the blocks a light job for `chunk` needs out of the world.
    ///
    /// The footprint is the 3x3 chunk ring at and above `chunk.y - 1`, bounded by
    /// the world column height. Missing chunks are air and are not copied.
    pub(crate) fn new(world: &World, chunk: ChunkCoord) -> Self {
        let mut cells = HashMap::new();
        for dx in -1..=1 {
            for dz in -1..=1 {
                for dy in -1..CHUNK_COLUMN {
                    let coordinate = chunk.offset(IVec3::new(dx, dy, dz));
                    if world.chunk(coordinate).is_some() {
                        cells.insert(coordinate, ChunkSnapshot::new(world, coordinate));
                    }
                }
            }
        }
        Self { cells }
    }
}

/// One light computation queued on the worker pool.
pub(crate) struct LightJob {
    pub(crate) chunk: ChunkCoord,
    pub(crate) revision: u64,
    pub(crate) snapshot: LightSnapshot,
}

/// A finished light field, tagged for stale-result rejection.
#[derive(Debug)]
pub(crate) struct LightResult {
    pub(crate) chunk: ChunkCoord,
    pub(crate) revision: u64,
    pub(crate) light: LightField,
    pub(crate) elapsed: Duration,
}

/// Sky and block light for a chunk plus one border cell.
///
/// Cells are indexed by chunk-local coordinates in `-1..=32` and stored as one
/// nibble per cell; the border is what meshing samples across chunk edges. The
/// block plane is omitted when the neighbourhood contains no lamp, which covers
/// every generated chunk.
#[derive(Clone, Debug)]
pub(crate) struct LightField {
    sky: PackedLight,
    block: Option<PackedLight>,
}

impl LightField {
    /// Bytes held by the packed planes that exist.
    pub(crate) fn resident_bytes(&self) -> usize {
        self.sky.resident_bytes() + self.block.as_ref().map_or(0, PackedLight::resident_bytes)
    }

    /// Sky light at a local coordinate in `-1..=32`.
    pub(crate) fn sky(&self, local: IVec3) -> u8 {
        self.sky.get(field_index(local))
    }

    /// Coarse BFS sky light for one `32³` section sample grid.
    ///
    /// Every non-solid sample above the highest solid sample in its column is
    /// open to the section sky and seeds [`MAX_LIGHT`]. Light then spreads
    /// six-connected through non-solid samples: `Water` keeps the value and
    /// every other non-solid sample loses one level, matching [`spread`] for L0
    /// chunks.
    ///
    /// The halo (`-1..=32`) is part of the BFS grid so the mesher's border
    /// samples are defined without touching another section. A halo sample
    /// reuses the nearest interior edge sample, so a solid edge stays solid and
    /// an open edge stays open; a halo column above the edge column's highest
    /// solid sample is seeded with [`MAX_LIGHT`]. This is conservative: the halo
    /// never invents light below an edge top, while the sky above a solid edge
    /// top (including `y = 32`) stays open.
    pub(crate) fn coarse(samples: &Chunk) -> Self {
        let stride = usize::try_from(COARSE_GRID).expect("coarse grid size fits");
        let plane = stride * stride;
        let cells = plane * stride;
        let columns = usize::try_from(SIZE * SIZE).expect("coarse columns fit in usize");
        let size = usize::try_from(SIZE).expect("section size fits");
        let storage = samples.shared_storage();

        // The outermost layer stays `Stone`, so BFS neighbours never need a
        // bounds check: every step off the halo lands in a solid sentinel.
        let mut blocks = vec![Block::Stone; cells];
        let mut ceiling = vec![-1_i32; columns];
        for x in 0..SIZE {
            for z in 0..SIZE {
                let column = usize::try_from(x * SIZE + z).expect("column fits");
                let mut source = usize::try_from(x * SIZE * SIZE + z).expect("sample fits");
                let mut destination = coarse_index(IVec3::new(x, 0, z));
                for y in 0..SIZE {
                    let sample = packed_block(&storage, source);
                    blocks[destination] = sample;
                    if sample.is_solid() {
                        ceiling[column] = y;
                    }
                    source += size;
                    destination += stride;
                }
            }
        }
        copy_halo_blocks(&mut blocks);

        let mut values = vec![0_u8; cells];
        let step = isize::try_from(stride).expect("coarse grid size fits");
        let slice = isize::try_from(plane).expect("coarse plane fits");
        let offsets = [1, -1, step, -step, slice, -slice];
        for x in -1..=SIZE {
            for z in -1..=SIZE {
                let edge_x = x.clamp(0, SIZE - 1);
                let edge_z = z.clamp(0, SIZE - 1);
                let column = usize::try_from(edge_x * SIZE + edge_z).expect("column fits");
                let first = ceiling[column] + 1;
                let mut index = coarse_index(IVec3::new(x, first, z));
                for _ in first..=SIZE {
                    values[index] = MAX_LIGHT;
                    index += stride;
                }
            }
        }
        // Only shadowed non-solid cells that touch an open-sky cell can start
        // a descent. Seeding just those keeps flat terrain out of the BFS.
        let mut buckets: [Vec<u32>; 16] = std::array::from_fn(|_| Vec::new());
        for (index, block) in blocks.iter().copied().enumerate() {
            if block.is_solid() || values[index] != 0 {
                continue;
            }
            if offsets
                .iter()
                .all(|offset| values[index.wrapping_add_signed(*offset)] != MAX_LIGHT)
            {
                continue;
            }
            let value = if block == Block::Water {
                MAX_LIGHT
            } else {
                MAX_LIGHT - 1
            };
            values[index] = value;
            buckets[usize::from(value)].push(u32::try_from(index).expect("coarse index fits"));
        }
        spread_coarse(&blocks, &mut values, &mut buckets, offsets);

        let mut field = PackedLight::new();
        let mut position = 0;
        for x in -1..=SIZE {
            let mut base = coarse_index(IVec3::new(x, -1, -1));
            for _ in -1..=SIZE {
                for (offset, _) in (-1..=SIZE).enumerate() {
                    field.set(position, values[base + offset]);
                    position += 1;
                }
                base += stride;
            }
        }
        Self {
            sky: field,
            block: None,
        }
    }

    /// Block light at a local coordinate in `-1..=32`.
    pub(crate) fn block(&self, local: IVec3) -> u8 {
        self.block
            .as_ref()
            .map_or(0, |plane| plane.get(field_index(local)))
    }
}

/// A plane of four-bit light values, two cells per byte.
#[derive(Clone, Debug, Default)]
struct PackedLight {
    cells: Box<[u8]>,
}

impl PackedLight {
    fn new() -> Self {
        let cells = usize::try_from(PADDED * PADDED * PADDED).expect("light field fits in usize");
        Self {
            cells: vec![0; cells.div_ceil(2)].into_boxed_slice(),
        }
    }

    fn resident_bytes(&self) -> usize {
        self.cells.len()
    }

    fn get(&self, index: usize) -> u8 {
        let byte = self.cells[index / 2];
        if index.is_multiple_of(2) {
            byte & 0x0F
        } else {
            byte >> 4
        }
    }

    fn set(&mut self, index: usize, value: u8) {
        debug_assert!(value <= MAX_LIGHT);
        let slot = &mut self.cells[index / 2];
        if index.is_multiple_of(2) {
            *slot = (*slot & 0xF0) | value;
        } else {
            *slot = (*slot & 0x0F) | (value << 4);
        }
    }
}

/// Computes light for one chunk from an immutable snapshot.
pub(crate) fn compute(snapshot: &LightSnapshot, chunk: ChunkCoord) -> LightField {
    let cells = usize::try_from(GRID * GRID * GRID).expect("light grid fits in usize");
    let base = chunk.world_origin() - IVec3::splat(SIZE);

    let mut blocks = vec![Block::Air; cells];
    let columns = usize::try_from(GRID * GRID).expect("light grid columns fit in usize");
    let column_stride = usize::try_from(GRID).expect("grid size fits");
    // Highest solid world y per grid column; `i32::MIN` means open to the sky.
    let mut ceiling = vec![i32::MIN; columns];
    let mut sky = vec![0_u8; cells];
    let mut block_light: Option<Vec<u8>> = None;
    let mut sky_queue = VecDeque::new();
    let mut block_queue = VecDeque::new();

    for (coordinate, data) in &snapshot.cells {
        let origin = coordinate.world_origin();
        let offset = origin - base;
        for cell in chunk_cells() {
            let block = data.block(cell);
            if block == Block::Air {
                continue;
            }
            let grid_cell = offset + cell;
            if block.is_solid() {
                let world = origin + cell;
                let x = usize::try_from(grid_cell.x).expect("in-grid coordinate fits");
                let z = usize::try_from(grid_cell.z).expect("in-grid coordinate fits");
                let top = &mut ceiling[x * column_stride + z];
                *top = (*top).max(world.y);
            }
            if !in_grid(grid_cell) {
                continue;
            }
            let index = grid_index(grid_cell);
            blocks[index] = block;
            if block == Block::Lamp {
                block_light.get_or_insert_with(|| vec![0_u8; cells])[index] = LAMP_LIGHT;
                block_queue.push_back((grid_cell, LAMP_LIGHT));
            }
        }
    }

    // Every cell above its column ceiling is open sky. Enqueue the ones that
    // touch a shadowed non-solid cell in the same pass; the old code scanned
    // the full grid again just to find them.
    for x in 0..GRID {
        for z in 0..GRID {
            let x_index = usize::try_from(x).expect("in-grid coordinate fits");
            let z_index = usize::try_from(z).expect("in-grid coordinate fits");
            let ceiling_y = ceiling[x_index * column_stride + z_index];
            let start = if ceiling_y == i32::MIN {
                0
            } else {
                (ceiling_y - base.y + 1).clamp(0, GRID)
            };
            for y in start..GRID {
                let cell = IVec3::new(x, y, z);
                sky[grid_index(cell)] = MAX_LIGHT;
                let boundary = NEIGHBOURS.iter().any(|offset| {
                    let neighbour = cell + *offset;
                    if !in_grid(neighbour) || blocks[grid_index(neighbour)].is_solid() {
                        return false;
                    }
                    let nx = usize::try_from(neighbour.x).expect("in-grid coordinate fits");
                    let nz = usize::try_from(neighbour.z).expect("in-grid coordinate fits");
                    let neighbour_ceiling = ceiling[nx * column_stride + nz];
                    neighbour_ceiling != i32::MIN && base.y + neighbour.y <= neighbour_ceiling
                });
                if boundary {
                    sky_queue.push_back((cell, MAX_LIGHT));
                }
            }
        }
    }

    spread(&blocks, &mut sky, &mut sky_queue);
    let block = block_light.map(|mut values| {
        spread(&blocks, &mut values, &mut block_queue);
        extract(&values)
    });

    LightField {
        sky: extract(&sky),
        block,
    }
}

/// Spreads queued values through six-connected non-solid cells.
///
/// A step into `Water` keeps the value; every other step loses one level.
fn spread(blocks: &[Block], values: &mut [u8], queue: &mut VecDeque<(IVec3, u8)>) {
    while let Some((cell, value)) = queue.pop_front() {
        if value == 0 {
            continue;
        }
        for offset in NEIGHBOURS {
            let neighbour = cell + offset;
            if !in_grid(neighbour) {
                continue;
            }
            let block = blocks[grid_index(neighbour)];
            if block.is_solid() {
                continue;
            }
            let next = if block == Block::Water {
                value
            } else {
                value - 1
            };
            let index = grid_index(neighbour);
            if next > values[index] {
                values[index] = next;
                queue.push_back((neighbour, next));
            }
        }
    }
}

/// Copies the nearest interior edge sample into the halo of `blocks`.
///
/// The six halo faces are filled once each; edge and corner halo samples take
/// the closest interior sample by clamping their in-plane coordinates. A solid
/// edge therefore stays solid and an open edge stays open.
fn copy_halo_blocks(blocks: &mut [Block]) {
    let last = SIZE - 1;
    let edge = |coordinate: i32| coordinate.clamp(0, last);
    for y in -1..=SIZE {
        for z in -1..=SIZE {
            let (ey, ez) = (edge(y), edge(z));
            blocks[coarse_index(IVec3::new(-1, y, z))] =
                blocks[coarse_index(IVec3::new(0, ey, ez))];
            blocks[coarse_index(IVec3::new(SIZE, y, z))] =
                blocks[coarse_index(IVec3::new(last, ey, ez))];
        }
    }
    for x in 0..SIZE {
        for y in -1..=SIZE {
            let ey = edge(y);
            blocks[coarse_index(IVec3::new(x, y, -1))] = blocks[coarse_index(IVec3::new(x, ey, 0))];
            blocks[coarse_index(IVec3::new(x, y, SIZE))] =
                blocks[coarse_index(IVec3::new(x, ey, last))];
        }
    }
    for x in 0..SIZE {
        for z in 0..SIZE {
            blocks[coarse_index(IVec3::new(x, -1, z))] = blocks[coarse_index(IVec3::new(x, 0, z))];
            blocks[coarse_index(IVec3::new(x, SIZE, z))] =
                blocks[coarse_index(IVec3::new(x, last, z))];
        }
    }
}

/// Spreads coarse sky values over the halo-padded section grid.
///
/// Mirrors [`spread`]: a step into `Water` keeps the value and every other step
/// loses one level. Buckets run from [`MAX_LIGHT`] down to one, so a cell is
/// finalized the first time it is reached with its highest value; the solid
/// sentinel layer around the grid replaces bounds checks.
fn spread_coarse(
    blocks: &[Block],
    values: &mut [u8],
    buckets: &mut [Vec<u32>; 16],
    offsets: [isize; 6],
) {
    for level in (1..=MAX_LIGHT).rev() {
        let bucket = usize::from(level);
        let mut index_in_bucket = 0;
        while index_in_bucket < buckets[bucket].len() {
            let index =
                usize::try_from(buckets[bucket][index_in_bucket]).expect("coarse index fits");
            index_in_bucket += 1;
            let next = level - 1;
            for offset in offsets {
                let neighbour = index.wrapping_add_signed(offset);
                let block = blocks[neighbour];
                if block.is_solid() {
                    continue;
                }
                let value = if block == Block::Water { level } else { next };
                if value > values[neighbour] {
                    values[neighbour] = value;
                    buckets[usize::from(value)]
                        .push(u32::try_from(neighbour).expect("coarse index fits"));
                }
            }
        }
    }
}

fn coarse_index(cell: IVec3) -> usize {
    let stride = usize::try_from(COARSE_GRID).expect("coarse grid size fits");
    let x = usize::try_from(cell.x + 2).expect("coarse coordinate fits");
    let y = usize::try_from(cell.y + 2).expect("coarse coordinate fits");
    let z = usize::try_from(cell.z + 2).expect("coarse coordinate fits");
    (x * stride + y) * stride + z
}

fn extract(values: &[u8]) -> PackedLight {
    let mut field = PackedLight::new();
    for local in grid_coords() {
        let cell = local + IVec3::splat(SIZE);
        field.set(field_index(local), values[grid_index(cell)]);
    }
    field
}

fn chunk_cells() -> impl Iterator<Item = IVec3> {
    (0..SIZE).flat_map(|x| (0..SIZE).flat_map(move |y| (0..SIZE).map(move |z| IVec3::new(x, y, z))))
}

fn grid_coords() -> impl Iterator<Item = IVec3> {
    (-1..=SIZE)
        .flat_map(|x| (-1..=SIZE).flat_map(move |y| (-1..=SIZE).map(move |z| IVec3::new(x, y, z))))
}

fn in_grid(cell: IVec3) -> bool {
    cell.cmpge(IVec3::ZERO).all() && cell.cmplt(IVec3::splat(GRID)).all()
}

fn grid_index(cell: IVec3) -> usize {
    let stride = usize::try_from(GRID).expect("grid size fits");
    let x = usize::try_from(cell.x).expect("in-grid coordinate fits");
    let y = usize::try_from(cell.y).expect("in-grid coordinate fits");
    let z = usize::try_from(cell.z).expect("in-grid coordinate fits");
    (x * stride + y) * stride + z
}

fn field_index(local: IVec3) -> usize {
    let padded = local + IVec3::ONE;
    let stride = usize::try_from(PADDED).expect("field size fits");
    let x = usize::try_from(padded.x).expect("field coordinate fits");
    let y = usize::try_from(padded.y).expect("field coordinate fits");
    let z = usize::try_from(padded.z).expect("field coordinate fits");
    (x * stride + y) * stride + z
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world_with(blocks: &[(IVec3, Block)]) -> World {
        let mut world = World::default();
        for &(coordinate, block) in blocks {
            world.set_block(coordinate, block);
        }
        world
    }

    fn chunk_at(world: IVec3) -> ChunkCoord {
        ChunkCoord::from_world(world).0
    }

    fn field(world: &World, chunk: ChunkCoord) -> LightField {
        compute(&LightSnapshot::new(world, chunk), chunk)
    }

    #[test]
    fn open_column_is_fully_lit() {
        let mut world = World::default();
        for x in 0..4 {
            for z in 0..4 {
                world.set_block(IVec3::new(x, 0, z), Block::Stone);
            }
        }
        let light = field(&world, chunk_at(IVec3::ZERO));

        assert_eq!(light.sky(IVec3::new(1, 1, 1)), MAX_LIGHT);
        assert_eq!(light.sky(IVec3::new(1, 31, 1)), MAX_LIGHT);
        assert_eq!(light.sky(IVec3::new(1, 0, 1)), 0);
    }

    #[test]
    fn packed_light_round_trips_every_value() {
        let mut plane = PackedLight::new();
        let cells = usize::try_from(PADDED * PADDED * PADDED).unwrap();

        assert_eq!(plane.cells.len(), cells.div_ceil(2));
        for index in 0..cells.div_ceil(2) {
            for value in 0..=MAX_LIGHT {
                plane.set(index, value);
                assert_eq!(plane.get(index), value, "{index}");
            }
        }
    }

    #[test]
    fn packed_fields_cover_the_halo() {
        let mut world = World::default();
        for x in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                world.set_block(IVec3::new(x, 0, z), Block::Stone);
            }
        }
        let chunk = ChunkCoord::new(0, 0, 0);
        let field = compute(&LightSnapshot::new(&world, chunk), chunk);

        assert_eq!(field.sky(IVec3::new(-1, 1, -1)), MAX_LIGHT);
        assert_eq!(field.sky(IVec3::new(SIZE, 1, SIZE)), MAX_LIGHT);
        assert_eq!(field.sky(IVec3::new(0, 0, 0)), 0);
        assert_eq!(field.block(IVec3::new(-1, -1, -1)), 0);
    }

    #[test]
    fn chunks_without_lamps_omit_the_block_plane() {
        let mut world = World::default();
        for x in 0..8 {
            for z in 0..8 {
                world.set_block(IVec3::new(x, 0, z), Block::Stone);
            }
        }
        let light = field(&world, chunk_at(IVec3::ZERO));

        let plane = usize::try_from(PADDED * PADDED * PADDED)
            .expect("plane fits")
            .div_ceil(2);
        assert_eq!(light.resident_bytes(), plane);
        assert_eq!(light.block(IVec3::new(3, 3, 3)), 0);
    }

    #[test]
    fn lamps_keep_the_block_plane() {
        let world = world_with(&[(IVec3::new(4, 4, 4), Block::Lamp)]);
        let light = field(&world, chunk_at(IVec3::ZERO));

        let plane = usize::try_from(PADDED * PADDED * PADDED)
            .expect("plane fits")
            .div_ceil(2);
        assert_eq!(light.resident_bytes(), plane * 2);
        assert_eq!(light.block(IVec3::new(5, 4, 4)), 13);
    }

    #[test]
    fn lamps_above_the_grid_omit_the_block_plane() {
        let world = world_with(&[(IVec3::new(4, 70, 4), Block::Lamp)]);
        let light = field(&world, chunk_at(IVec3::ZERO));

        let plane = usize::try_from(PADDED * PADDED * PADDED)
            .expect("plane fits")
            .div_ceil(2);
        assert_eq!(light.resident_bytes(), plane);
        assert_eq!(light.block(IVec3::new(4, 1, 4)), 0);
    }

    #[test]
    fn a_light_snapshot_uses_the_column_footprint() {
        let mut world = World::default();
        for y in 0..CHUNK_COLUMN {
            world.set_block(IVec3::new(1, y * CHUNK_SIZE, 1), Block::Stone);
        }
        world.set_block(IVec3::new(-1, 0, 0), Block::Stone);
        world.set_block(IVec3::new(100, 0, 0), Block::Stone);

        let snapshot = LightSnapshot::new(&world, ChunkCoord::new(0, 0, 0));

        for y in 0..CHUNK_COLUMN {
            assert!(
                snapshot.cells.contains_key(&ChunkCoord::new(0, y, 0)),
                "y {y}"
            );
        }
        assert!(snapshot.cells.contains_key(&ChunkCoord::new(-1, 0, 0)));
        assert_eq!(snapshot.cells.len(), CHUNK_COLUMN as usize + 1);

        let top = LightSnapshot::new(&world, ChunkCoord::new(0, CHUNK_COLUMN - 1, 0));
        assert!(
            top.cells
                .contains_key(&ChunkCoord::new(0, CHUNK_COLUMN - 2, 0))
        );
        assert!(
            top.cells
                .contains_key(&ChunkCoord::new(0, CHUNK_COLUMN - 1, 0))
        );
    }

    #[test]
    fn sky_light_falls_off_under_an_overhang() {
        let mut world = World::default();
        for x in -32..64 {
            for z in -32..64 {
                world.set_block(IVec3::new(x, 4, z), Block::Stone);
            }
        }
        world.set_block(IVec3::new(0, 4, 0), Block::Air);

        let light = field(&world, chunk_at(IVec3::ZERO));

        assert_eq!(light.sky(IVec3::new(0, 3, 0)), 15);
        assert_eq!(light.sky(IVec3::new(1, 3, 0)), 14);
        assert_eq!(light.sky(IVec3::new(2, 3, 0)), 13);
        assert_eq!(light.sky(IVec3::new(3, 3, 0)), 12);
    }

    #[test]
    fn enclosed_caves_are_dark() {
        let mut world = World::default();
        for x in 0..8 {
            for y in 0..8 {
                for z in 0..8 {
                    let shell = x == 0 || y == 0 || z == 0 || x == 7 || y == 7 || z == 7;
                    world.set_block(
                        IVec3::new(x, y, z),
                        if shell { Block::Stone } else { Block::Air },
                    );
                }
            }
        }

        let light = field(&world, chunk_at(IVec3::ZERO));

        assert_eq!(light.sky(IVec3::new(3, 3, 3)), 0);
        assert_eq!(light.block(IVec3::new(3, 3, 3)), 0);
    }

    #[test]
    fn lamp_light_falls_off_with_distance() {
        let world = world_with(&[(IVec3::new(4, 4, 4), Block::Lamp)]);
        let light = field(&world, chunk_at(IVec3::ZERO));

        assert_eq!(light.block(IVec3::new(4, 4, 4)), LAMP_LIGHT);
        assert_eq!(light.block(IVec3::new(5, 4, 4)), 13);
        assert_eq!(light.block(IVec3::new(6, 4, 4)), 12);
        assert_eq!(light.block(IVec3::new(10, 4, 4)), 8);
        assert_eq!(light.sky(IVec3::new(4, 4, 4)), 0);
    }

    #[test]
    fn water_passes_light_without_attenuation() {
        let mut world = World::default();
        for x in 0..4 {
            for z in 0..4 {
                for y in 1..4 {
                    world.set_block(IVec3::new(x, y, z), Block::Water);
                }
                world.set_block(IVec3::new(x, 0, z), Block::Lamp);
            }
        }

        let light = field(&world, chunk_at(IVec3::ZERO));

        assert_eq!(light.sky(IVec3::new(1, 3, 1)), MAX_LIGHT);
        assert_eq!(light.block(IVec3::new(1, 1, 1)), 14);
        assert_eq!(light.block(IVec3::new(1, 3, 1)), 14);
    }

    #[test]
    fn light_crosses_chunk_borders_into_the_border_field() {
        let world = world_with(&[(IVec3::new(31, 0, 0), Block::Lamp)]);
        let neighbour = field(&world, chunk_at(IVec3::new(32, 0, 0)));

        assert_eq!(neighbour.block(IVec3::new(-1, 0, 0)), 14);
        assert_eq!(neighbour.block(IVec3::new(0, 0, 0)), 13);
        assert_eq!(neighbour.block(IVec3::new(1, 0, 0)), 12);
    }

    #[test]
    fn sky_light_propagates_across_a_chunk_border() {
        let mut world = World::default();
        for x in -32..32 {
            for z in -32..64 {
                world.set_block(IVec3::new(x, 4, z), Block::Stone);
            }
        }
        let left = field(&world, chunk_at(IVec3::new(31, 0, 0)));

        assert_eq!(left.sky(IVec3::new(32, 3, 0)), 15);
        assert_eq!(left.sky(IVec3::new(31, 3, 0)), 14);
        assert_eq!(left.sky(IVec3::new(30, 3, 0)), 13);
    }

    #[test]
    fn light_levels_stay_in_range() {
        let mut world = World::default();
        world.set_block(IVec3::new(1, 1, 1), Block::Lamp);
        for x in 0..8 {
            for z in 0..8 {
                world.set_block(IVec3::new(x, 0, z), Block::Stone);
            }
        }

        let light = field(&world, chunk_at(IVec3::ZERO));

        for x in -1..=32 {
            for y in -1..=32 {
                for z in -1..=32 {
                    let local = IVec3::new(x, y, z);
                    assert!(light.sky(local) <= MAX_LIGHT);
                    assert!(light.block(local) <= MAX_LIGHT);
                }
            }
        }
    }
}
