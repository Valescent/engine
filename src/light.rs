//! Voxel sky and block light: 4-bit storage, sources, and BFS propagation.
//!
//! Light is derived state. It is recomputed from blocks and never saved.
//! Propagation runs over a 3x3x3 chunk neighbourhood so values on the one-cell
//! border of a chunk are exact, then the centre chunk plus border is extracted
//! as a [`LightField`].

use std::collections::{HashMap, VecDeque};

use glam::IVec3;

use crate::block::Block;
use crate::world::{CHUNK_SIZE, ChunkCoord, World};

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

const NEIGHBOURS: [IVec3; 6] = [
    IVec3::X,
    IVec3::NEG_X,
    IVec3::Y,
    IVec3::NEG_Y,
    IVec3::Z,
    IVec3::NEG_Z,
];

/// Highest solid block per world column, or absent when the column is empty.
pub(crate) type SkyCeiling = HashMap<(i32, i32), i32>;

/// Sky and block light for a chunk plus one border cell.
///
/// Cells are indexed by chunk-local coordinates in `-1..=32`; the border is what
/// meshing samples across chunk edges.
#[derive(Clone, Debug)]
pub(crate) struct LightField {
    sky: Box<[u8]>,
    block: Box<[u8]>,
}

impl LightField {
    /// Sky light at a local coordinate in `-1..=32`.
    pub(crate) fn sky(&self, local: IVec3) -> u8 {
        self.sky[field_index(local)]
    }

    /// Block light at a local coordinate in `-1..=32`.
    pub(crate) fn block(&self, local: IVec3) -> u8 {
        self.block[field_index(local)]
    }
}

/// Computes light for one chunk from the world's blocks and sky ceiling.
pub(crate) fn compute(world: &World, chunk: ChunkCoord, ceiling: &SkyCeiling) -> LightField {
    let cells = usize::try_from(GRID * GRID * GRID).expect("light grid fits in usize");
    let base = chunk.world_origin() - IVec3::splat(SIZE);

    let mut blocks = vec![Block::Air; cells];
    for dx in -1..=1 {
        for dy in -1..=1 {
            for dz in -1..=1 {
                let neighbour = chunk.offset(IVec3::new(dx, dy, dz));
                let Some(data) = world.chunk(neighbour) else {
                    continue;
                };
                let offset = data_origin_offset(neighbour, base);
                for (&local, &block) in &data.blocks {
                    let cell = offset + local;
                    if in_grid(cell) {
                        blocks[grid_index(cell)] = block;
                    }
                }
            }
        }
    }

    let mut sky = vec![0_u8; cells];
    let mut block_light = vec![0_u8; cells];
    let mut sky_queue = VecDeque::new();
    let mut block_queue = VecDeque::new();

    for x in 0..GRID {
        for z in 0..GRID {
            let column = (base.x + x, base.z + z);
            let ceiling_y = ceiling.get(&column).copied();
            for y in 0..GRID {
                let cell = IVec3::new(x, y, z);
                let index = grid_index(cell);
                let block = blocks[index];
                if block.is_solid() {
                    if block == Block::Lamp {
                        block_light[index] = LAMP_LIGHT;
                        block_queue.push_back((cell, LAMP_LIGHT));
                    }
                    continue;
                }
                let world_y = base.y + y;
                if ceiling_y.is_none_or(|top| world_y > top) {
                    sky[index] = MAX_LIGHT;
                }
            }
        }
    }

    for x in 0..GRID {
        for y in 0..GRID {
            for z in 0..GRID {
                let cell = IVec3::new(x, y, z);
                if sky[grid_index(cell)] != MAX_LIGHT {
                    continue;
                }
                if NEIGHBOURS.iter().any(|offset| {
                    let neighbour = cell + *offset;
                    in_grid(neighbour)
                        && !blocks[grid_index(neighbour)].is_solid()
                        && sky[grid_index(neighbour)] != MAX_LIGHT
                }) {
                    sky_queue.push_back((cell, MAX_LIGHT));
                }
            }
        }
    }

    spread(&blocks, &mut sky, &mut sky_queue);
    spread(&blocks, &mut block_light, &mut block_queue);

    LightField {
        sky: extract(&sky),
        block: extract(&block_light),
    }
}

/// Highest solid block in every column of the world.
pub(crate) fn sky_ceiling(world: &World) -> SkyCeiling {
    let mut ceiling = SkyCeiling::new();
    for (chunk, data) in world.iter_chunks() {
        let origin = chunk.world_origin();
        for (&local, &block) in &data.blocks {
            if !block.is_solid() {
                continue;
            }
            let world = origin + local;
            ceiling
                .entry((world.x, world.z))
                .and_modify(|top: &mut i32| *top = (*top).max(world.y))
                .or_insert(world.y);
        }
    }
    ceiling
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

fn extract(values: &[u8]) -> Box<[u8]> {
    let cells = usize::try_from(PADDED * PADDED * PADDED).expect("light field fits in usize");
    let mut field = vec![0_u8; cells].into_boxed_slice();
    for local in grid_coords() {
        let cell = local + IVec3::splat(SIZE);
        field[field_index(local)] = values[grid_index(cell)];
    }
    field
}

fn grid_coords() -> impl Iterator<Item = IVec3> {
    (-1..=SIZE)
        .flat_map(|x| (-1..=SIZE).flat_map(move |y| (-1..=SIZE).map(move |z| IVec3::new(x, y, z))))
}

fn data_origin_offset(chunk: ChunkCoord, base: IVec3) -> IVec3 {
    chunk.world_origin() - base
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
        compute(world, chunk, &sky_ceiling(world))
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
