//! Greedy chunk meshing, LOD downsampling, and LOD selection.
//!
//! Chunk meshes are built in chunk-local space; the renderer adds each chunk's
//! origin. One greedy mesher serves both LODs: LOD0 works on `32` cells of one
//! block, LOD1 on `16` cells of `2 × 2 × 2` blocks.

use std::collections::HashMap;

use glam::IVec3;

use crate::block::Block;
use crate::voxel::Vertex;
use crate::world::{CHUNK_SIZE, ChunkCoord, World};

/// Distance in chunk units at or below which a chunk renders at LOD0.
pub(crate) const LOD0_DISTANCE: f32 = 4.0;

/// Distance in chunk units at or above which a chunk renders at LOD1.
pub(crate) const LOD1_DISTANCE: f32 = 8.0;

/// Blocks per LOD1 cell on each axis.
const LOD1_SCALE: i32 = 2;

/// LOD1 cells per chunk axis.
const LOD1_SIZE: i32 = CHUNK_SIZE / LOD1_SCALE;

/// Two rendering levels of detail.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum Lod {
    Lod0,
    Lod1,
}

impl Lod {
    /// Blocks per cell, used for positions and UVs.
    pub(crate) fn scale(self) -> f32 {
        match self {
            Self::Lod0 => 1.0,
            Self::Lod1 => 2.0,
        }
    }

    /// Cells per chunk axis.
    fn cell_count(self) -> i32 {
        match self {
            Self::Lod0 => CHUNK_SIZE,
            Self::Lod1 => LOD1_SIZE,
        }
    }
}

/// Picks a LOD from the current one and the chunk distance, with hysteresis.
///
/// `current` is `None` on the first selection. Distances between the two
/// thresholds retain the current LOD so boundaries do not flicker.
pub(crate) fn select_lod(current: Option<Lod>, distance: f32) -> Lod {
    let Some(current) = current else {
        return if distance >= LOD1_DISTANCE {
            Lod::Lod1
        } else {
            Lod::Lod0
        };
    };

    if distance <= LOD0_DISTANCE {
        Lod::Lod0
    } else if distance >= LOD1_DISTANCE {
        Lod::Lod1
    } else {
        current
    }
}

/// Immutable full-resolution copy of one chunk's blocks.
#[derive(Clone, Debug, Default)]
pub(crate) struct ChunkSnapshot {
    blocks: Box<[Block]>,
}

impl ChunkSnapshot {
    /// Copies a chunk out of the world. A missing chunk becomes all air.
    pub(crate) fn new(world: &World, chunk: ChunkCoord) -> Self {
        let cells = usize::try_from(CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE).expect("grid fits");
        let mut blocks = vec![Block::Air; cells].into_boxed_slice();
        if let Some(data) = world.chunk(chunk) {
            for (&local, &block) in &data.blocks {
                if in_chunk(local) {
                    blocks[index(local)] = block;
                }
            }
        }
        Self { blocks }
    }

    /// Returns the block at a chunk-local coordinate, or air outside the chunk.
    pub(crate) fn block(&self, local: IVec3) -> Block {
        if in_chunk(local) {
            self.blocks[index(local)]
        } else {
            Block::Air
        }
    }
}

/// The one-cell border around a chunk, keyed by the owning chunk's local
/// coordinates (so a `-X` neighbour lands at `x = -1`).
#[derive(Clone, Debug, Default)]
pub(crate) struct BorderBlocks {
    blocks: HashMap<IVec3, Block>,
}

impl BorderBlocks {
    /// Copies the solid border cells of the six neighbouring chunks.
    pub(crate) fn new(world: &World, chunk: ChunkCoord) -> Self {
        let mut blocks = HashMap::new();
        for normal in FACE_NORMALS {
            let Some(data) = world.chunk(chunk.offset(normal)) else {
                continue;
            };
            for (&local, &block) in &data.blocks {
                if block.is_solid() && on_shared_face(local, normal) {
                    blocks.insert(local + normal * CHUNK_SIZE, block);
                }
            }
        }
        Self { blocks }
    }

    /// Returns the border block at an out-of-chunk local coordinate.
    pub(crate) fn block(&self, local: IVec3) -> Block {
        self.blocks.get(&local).copied().unwrap_or(Block::Air)
    }
}

/// Builds one non-indexed greedy mesh for a chunk snapshot.
pub(crate) fn build_mesh(snapshot: &ChunkSnapshot, border: &BorderBlocks, lod: Lod) -> Vec<Vertex> {
    match lod {
        Lod::Lod0 => greedy(lod.cell_count(), lod.scale(), |cell| {
            if in_chunk(cell) {
                snapshot.block(cell)
            } else {
                border.block(cell)
            }
        }),
        Lod::Lod1 => {
            let cells = downsample(snapshot);
            greedy(lod.cell_count(), lod.scale(), |cell| {
                if in_grid(cell, LOD1_SIZE) {
                    cells[index_in(cell, LOD1_SIZE)]
                } else {
                    Block::Air
                }
            })
        }
    }
}

/// Downsamples a chunk to one block per LOD1 cell: solid when any source block
/// is solid, typed by the topmost solid source, then lowest X, then lowest Z.
fn downsample(snapshot: &ChunkSnapshot) -> Box<[Block]> {
    let cells = usize::try_from(LOD1_SIZE * LOD1_SIZE * LOD1_SIZE).expect("grid fits");
    let mut grid = vec![Block::Air; cells].into_boxed_slice();
    for cell in grid_coords(LOD1_SIZE) {
        grid[index_in(cell, LOD1_SIZE)] = downsample_cell(snapshot, cell);
    }
    grid
}

fn downsample_cell(snapshot: &ChunkSnapshot, cell: IVec3) -> Block {
    let base = cell * LOD1_SCALE;
    for y in (0..LOD1_SCALE).rev() {
        for x in 0..LOD1_SCALE {
            for z in 0..LOD1_SCALE {
                let block = snapshot.block(base + IVec3::new(x, y, z));
                if block.is_solid() {
                    return block;
                }
            }
        }
    }
    Block::Air
}

/// Face order from the v0.0.5 spec: `+X`, `-X`, `+Y`, `-Y`, `+Z`, `-Z`.
const FACE_NORMALS: [IVec3; 6] = [
    IVec3::X,
    IVec3::NEG_X,
    IVec3::Y,
    IVec3::NEG_Y,
    IVec3::Z,
    IVec3::NEG_Z,
];

/// Builds a width-first greedy mesh of the solid cells returned by `block_at`.
fn greedy(n: i32, scale: f32, block_at: impl Fn(IVec3) -> Block) -> Vec<Vertex> {
    let cells = usize::try_from(n * n).expect("mask fits");
    let mut vertices = Vec::new();

    for (face, normal) in FACE_NORMALS.into_iter().enumerate() {
        for slice in 0..n {
            let mut mask: Vec<Option<MaskKey>> = vec![None; cells];
            for v in 0..n {
                for u in 0..n {
                    let cell = cell_coord(face, slice, u, v);
                    let block = block_at(cell);
                    if !block.is_solid() || block_at(cell + normal).is_solid() {
                        continue;
                    }
                    mask[mask_index(u, v, n)] = Some(MaskKey {
                        block,
                        layer: face_layer(block, normal),
                        ao: corner_ao(&block_at, cell, face),
                    });
                }
            }

            for v in 0..n {
                for u in 0..n {
                    let Some(key) = mask[mask_index(u, v, n)] else {
                        continue;
                    };

                    let mut width = 1;
                    while u + width < n && mask[mask_index(u + width, v, n)] == Some(key) {
                        width += 1;
                    }

                    let mut height = 1;
                    'height: while v + height < n {
                        for offset in 0..width {
                            if mask[mask_index(u + offset, v + height, n)] != Some(key) {
                                break 'height;
                            }
                        }
                        height += 1;
                    }

                    for row in 0..height {
                        for column in 0..width {
                            mask[mask_index(u + column, v + row, n)] = None;
                        }
                    }

                    push_quad(
                        &mut vertices,
                        face,
                        normal,
                        (slice, u, v, width, height),
                        scale,
                        key.layer,
                    );
                }
            }
        }
    }

    vertices
}

/// Greedy merge key for one exposed cell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MaskKey {
    block: Block,
    layer: u32,
    /// Ambient occlusion at the four cell corners, in quad corner order.
    ao: [u8; 4],
}

/// Ambient occlusion at the four corners of one exposed face.
///
/// Each corner inspects the outside cell and its two edge neighbours plus the
/// diagonal in the plane of the face. Two solid edges fully occlude the corner.
fn corner_ao(block_at: &impl Fn(IVec3) -> Block, cell: IVec3, face: usize) -> [u8; 4] {
    let outside = cell + FACE_NORMALS[face];
    let (u_axis, v_axis) = face_axes(face);
    [(-1, -1), (-1, 1), (1, 1), (1, -1)].map(|(su, sv)| {
        let side_a = block_at(outside + u_axis * su).is_solid();
        let side_b = block_at(outside + v_axis * sv).is_solid();
        let diagonal = block_at(outside + u_axis * su + v_axis * sv).is_solid();
        if side_a && side_b {
            0
        } else {
            3 - (u8::from(side_a) + u8::from(side_b) + u8::from(diagonal))
        }
    })
}

fn face_axes(face: usize) -> (IVec3, IVec3) {
    match face {
        0 | 1 => (IVec3::Z, IVec3::Y),
        2 | 3 => (IVec3::X, IVec3::Z),
        _ => (IVec3::X, IVec3::Y),
    }
}

/// Cell coordinate for `(slice, u, v)` on a face, from the spec's table.
fn cell_coord(face: usize, slice: i32, u: i32, v: i32) -> IVec3 {
    match face {
        0 | 1 => IVec3::new(slice, v, u),
        2 | 3 => IVec3::new(u, slice, v),
        _ => IVec3::new(u, v, slice),
    }
}

fn mask_index(u: i32, v: i32, n: i32) -> usize {
    usize::try_from(v * n + u).expect("mask index fits")
}

fn face_layer(block: Block, normal: IVec3) -> u32 {
    let layers = block.layers().expect("solid blocks have texture layers");
    match normal.y.cmp(&0) {
        std::cmp::Ordering::Greater => layers.top,
        std::cmp::Ordering::Less => layers.bottom,
        std::cmp::Ordering::Equal => layers.side,
    }
}

/// Emits one merged quad with UVs in block units.
#[allow(clippy::cast_precision_loss)]
fn push_quad(
    vertices: &mut Vec<Vertex>,
    face: usize,
    normal: IVec3,
    quad: (i32, i32, i32, i32, i32),
    scale: f32,
    layer: u32,
) {
    let (slice, u0, v0, width, height) = quad;
    let corners = quad_corners(face, slice, u0, v0, width, height);
    let us = width as f32 * scale;
    let vs = height as f32 * scale;
    let uvs = [[0.0, 0.0], [0.0, vs], [us, vs], [us, 0.0]];
    let normal = normal.as_vec3().to_array();

    for index in [0, 1, 2, 0, 2, 3] {
        vertices.push(Vertex {
            position: corners[index].map(|coordinate| coordinate * scale),
            normal,
            uv: uvs[index],
            layer,
        });
    }
}

/// Quad corners in cell units, in the spec's corner order.
#[allow(clippy::cast_precision_loss)]
fn quad_corners(
    face: usize,
    slice: i32,
    u0: i32,
    v0: i32,
    width: i32,
    height: i32,
) -> [[f32; 3]; 4] {
    let s = slice as f32;
    let u = u0 as f32;
    let v = v0 as f32;
    let right = (u0 + width) as f32;
    let top = (v0 + height) as f32;
    let forward = s + 1.0;

    match face {
        0 => [
            [forward, v, u],
            [forward, top, u],
            [forward, top, right],
            [forward, v, right],
        ],
        1 => [[s, v, right], [s, top, right], [s, top, u], [s, v, u]],
        2 => [
            [u, forward, v],
            [u, forward, top],
            [right, forward, top],
            [right, forward, v],
        ],
        3 => [[u, s, top], [u, s, v], [right, s, v], [right, s, top]],
        4 => [
            [right, v, forward],
            [right, top, forward],
            [u, top, forward],
            [u, v, forward],
        ],
        _ => [[u, v, s], [u, top, s], [right, top, s], [right, v, s]],
    }
}

fn in_chunk(local: IVec3) -> bool {
    in_grid(local, CHUNK_SIZE)
}

fn in_grid(local: IVec3, size: i32) -> bool {
    local.cmpge(IVec3::ZERO).all() && local.cmplt(IVec3::splat(size)).all()
}

fn index(local: IVec3) -> usize {
    index_in(local, CHUNK_SIZE)
}

fn index_in(local: IVec3, size: i32) -> usize {
    let stride = usize::try_from(size).expect("grid size fits");
    let x = usize::try_from(local.x).expect("in-grid coordinate fits");
    let y = usize::try_from(local.y).expect("in-grid coordinate fits");
    let z = usize::try_from(local.z).expect("in-grid coordinate fits");
    (x * stride + y) * stride + z
}

fn grid_coords(size: i32) -> impl Iterator<Item = IVec3> {
    (0..size)
        .flat_map(move |x| (0..size).flat_map(move |y| (0..size).map(move |z| IVec3::new(x, y, z))))
}

fn on_shared_face(local: IVec3, normal: IVec3) -> bool {
    let shared = |coordinate: i32, direction: i32| {
        if direction < 0 {
            coordinate == CHUNK_SIZE - 1
        } else {
            coordinate == 0
        }
    };
    if normal.x != 0 {
        shared(local.x, normal.x)
    } else if normal.y != 0 {
        shared(local.y, normal.y)
    } else {
        shared(local.z, normal.z)
    }
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

    fn mesh(world: &World, chunk: ChunkCoord, lod: Lod) -> Vec<Vertex> {
        build_mesh(
            &ChunkSnapshot::new(world, chunk),
            &BorderBlocks::new(world, chunk),
            lod,
        )
    }

    /// One expected quad: `(face, slice, u0, v0, width, height, scale, layer)`.
    type Quad = (usize, i32, i32, i32, i32, i32, f32, u32);

    #[allow(clippy::cast_precision_loss)]
    fn quad(spec: Quad) -> Vec<Vertex> {
        let (face, slice, u0, v0, width, height, scale, layer) = spec;
        let normal = FACE_NORMALS[face].as_vec3().to_array();
        let us = width as f32 * scale;
        let vs = height as f32 * scale;
        let uvs = [[0.0, 0.0], [0.0, vs], [us, vs], [us, 0.0]];
        let corners = quad_corners(face, slice, u0, v0, width, height);
        [0, 1, 2, 0, 2, 3]
            .map(|index| Vertex {
                position: corners[index].map(|coordinate| coordinate * scale),
                normal,
                uv: uvs[index],
                layer,
            })
            .to_vec()
    }

    fn expected(quads: &[Quad]) -> Vec<Vertex> {
        quads.iter().flat_map(|&spec| quad(spec)).collect()
    }

    #[test]
    fn one_voxel_matches_the_spec_corner_table() {
        let world = world_with(&[(IVec3::new(3, 4, 5), Block::Stone)]);
        let vertices = mesh(&world, chunk_at(IVec3::new(3, 4, 5)), Lod::Lod0);

        assert_eq!(
            vertices,
            expected(&[
                (0, 3, 5, 4, 1, 1, 1.0, 0),
                (1, 3, 5, 4, 1, 1, 1.0, 0),
                (2, 4, 3, 5, 1, 1, 1.0, 0),
                (3, 4, 3, 5, 1, 1, 1.0, 0),
                (4, 5, 3, 4, 1, 1, 1.0, 0),
                (5, 5, 3, 4, 1, 1, 1.0, 0),
            ])
        );
        assert_eq!(vertices.len(), 36);
    }

    #[test]
    fn two_adjacent_voxels_merge_into_six_quads() {
        let world = world_with(&[(IVec3::ZERO, Block::Stone), (IVec3::X, Block::Stone)]);

        assert_eq!(mesh(&world, chunk_at(IVec3::ZERO), Lod::Lod0).len(), 36);
    }

    #[test]
    fn three_voxels_in_a_line_merge_into_six_quads() {
        let world = world_with(&[
            (IVec3::ZERO, Block::Stone),
            (IVec3::X, Block::Stone),
            (IVec3::new(2, 0, 0), Block::Stone),
        ]);

        assert_eq!(mesh(&world, chunk_at(IVec3::ZERO), Lod::Lod0).len(), 36);
    }

    #[test]
    fn slabs_and_cubes_merge_into_six_quads() {
        let mut slab = World::default();
        let mut cube = World::default();
        for x in 0..2 {
            for z in 0..2 {
                slab.set_block(IVec3::new(x, 0, z), Block::Dirt);
                for y in 0..2 {
                    cube.set_block(IVec3::new(x, y, z), Block::Dirt);
                }
            }
        }

        assert_eq!(mesh(&slab, chunk_at(IVec3::ZERO), Lod::Lod0).len(), 36);
        assert_eq!(mesh(&cube, chunk_at(IVec3::ZERO), Lod::Lod0).len(), 36);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn merged_quad_uses_block_unit_uvs_and_the_face_layer() {
        let mut world = World::default();
        for x in 0..2 {
            for z in 0..3 {
                world.set_block(IVec3::new(x, 0, z), Block::Grass);
            }
        }
        let vertices = mesh(&world, chunk_at(IVec3::ZERO), Lod::Lod0);

        let top: Vec<_> = vertices
            .iter()
            .filter(|vertex| vertex.normal == [0.0, 1.0, 0.0])
            .collect();
        assert_eq!(top.len(), 6);
        let uvs: Vec<_> = top.iter().map(|vertex| vertex.uv).collect();
        assert_eq!(
            uvs,
            vec![
                [0.0, 0.0],
                [0.0, 3.0],
                [2.0, 3.0],
                [0.0, 0.0],
                [2.0, 3.0],
                [2.0, 0.0]
            ]
        );
        assert!(top.iter().all(|vertex| vertex.layer == 2));
    }

    #[test]
    fn adjacent_voxels_across_a_chunk_border_emit_five_quads_each() {
        let world = world_with(&[
            (IVec3::new(31, 0, 0), Block::Stone),
            (IVec3::new(32, 0, 0), Block::Stone),
        ]);

        assert_eq!(
            mesh(&world, chunk_at(IVec3::new(31, 0, 0)), Lod::Lod0).len(),
            30
        );
        assert_eq!(
            mesh(&world, chunk_at(IVec3::new(32, 0, 0)), Lod::Lod0).len(),
            30
        );
    }

    #[test]
    fn greedy_uses_fewer_vertices_than_the_naive_mesher_on_flat_ground() {
        let mut world = World::default();
        for x in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                world.set_block(IVec3::new(x, 2, z), Block::Grass);
                world.set_block(IVec3::new(x, 1, z), Block::Dirt);
                world.set_block(IVec3::new(x, 0, z), Block::Stone);
            }
        }
        let chunk = chunk_at(IVec3::ZERO);

        let greedy = mesh(&world, chunk, Lod::Lod0).len();
        let naive = crate::voxel::build_chunk_mesh(&world, chunk).len();

        assert!(greedy < naive, "greedy {greedy} naive {naive}");
    }

    #[test]
    fn lod1_downsample_is_solid_when_any_source_is_solid() {
        let world = world_with(&[(IVec3::new(1, 1, 1), Block::Stone)]);
        let snapshot = ChunkSnapshot::new(&world, chunk_at(IVec3::ZERO));
        let cells = downsample(&snapshot);

        assert_eq!(cells[index_in(IVec3::ZERO, LOD1_SIZE)], Block::Stone);
        assert_eq!(cells[index_in(IVec3::X, LOD1_SIZE)], Block::Air);
    }

    #[test]
    fn lod1_downsample_takes_the_topmost_block_then_lowest_x_then_z() {
        let world = world_with(&[
            (IVec3::new(0, 0, 0), Block::Stone),
            (IVec3::new(0, 1, 0), Block::Dirt),
            (IVec3::new(1, 1, 0), Block::Grass),
            (IVec3::new(0, 0, 1), Block::Wood),
        ]);
        let snapshot = ChunkSnapshot::new(&world, chunk_at(IVec3::ZERO));

        assert_eq!(
            downsample(&snapshot)[index_in(IVec3::ZERO, LOD1_SIZE)],
            Block::Dirt
        );

        let mut tie = World::default();
        tie.set_block(IVec3::new(1, 1, 1), Block::Leaves);
        tie.set_block(IVec3::new(0, 1, 1), Block::Wood);
        let tie = downsample(&ChunkSnapshot::new(&tie, chunk_at(IVec3::ZERO)));
        assert_eq!(tie[index_in(IVec3::ZERO, LOD1_SIZE)], Block::Wood);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn lod1_scales_positions_and_uvs_by_two() {
        let world = world_with(&[(IVec3::new(2, 2, 2), Block::Stone)]);
        let vertices = mesh(&world, chunk_at(IVec3::new(2, 2, 2)), Lod::Lod1);

        assert_eq!(
            vertices,
            expected(&[
                (0, 1, 1, 1, 1, 1, 2.0, 0),
                (1, 1, 1, 1, 1, 1, 2.0, 0),
                (2, 1, 1, 1, 1, 1, 2.0, 0),
                (3, 1, 1, 1, 1, 1, 2.0, 0),
                (4, 1, 1, 1, 1, 1, 2.0, 0),
                (5, 1, 1, 1, 1, 1, 2.0, 0),
            ])
        );
        let uv = vertices[1].uv;
        assert_eq!(uv, [0.0, 2.0]);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn lod1_emits_boundary_faces_even_next_to_solid_neighbours() {
        let world = world_with(&[
            (IVec3::new(31, 0, 0), Block::Stone),
            (IVec3::new(32, 0, 0), Block::Stone),
        ]);
        let chunk = chunk_at(IVec3::new(31, 0, 0));

        let lod0 = mesh(&world, chunk, Lod::Lod0);
        let lod1 = mesh(&world, chunk, Lod::Lod1);

        assert_eq!(lod0.len(), 30);
        assert_ne!(lod1.len(), 0);
        assert_eq!(
            lod1.iter()
                .filter(|vertex| vertex.normal == [-1.0, 0.0, 0.0])
                .count(),
            6
        );
    }

    #[test]
    fn open_corners_have_full_ambient_occlusion() {
        let world = world_with(&[(IVec3::ZERO, Block::Stone)]);
        let chunk = chunk_at(IVec3::ZERO);
        let snapshot = ChunkSnapshot::new(&world, chunk);
        let border = BorderBlocks::new(&world, chunk);
        let block_at = |cell: IVec3| {
            if in_chunk(cell) {
                snapshot.block(cell)
            } else {
                border.block(cell)
            }
        };

        assert_eq!(corner_ao(&block_at, IVec3::ZERO, 2), [3, 3, 3, 3]);
    }

    #[test]
    fn one_solid_edge_removes_one_occlusion_level() {
        let world = world_with(&[
            (IVec3::ZERO, Block::Stone),
            (IVec3::new(1, 1, 0), Block::Stone),
        ]);
        let chunk = chunk_at(IVec3::ZERO);
        let snapshot = ChunkSnapshot::new(&world, chunk);
        let border = BorderBlocks::new(&world, chunk);
        let block_at = |cell: IVec3| {
            if in_chunk(cell) {
                snapshot.block(cell)
            } else {
                border.block(cell)
            }
        };

        assert_eq!(corner_ao(&block_at, IVec3::ZERO, 2), [3, 3, 2, 2]);
    }

    #[test]
    fn two_solid_edges_fully_occlude_a_corner() {
        let world = world_with(&[
            (IVec3::ZERO, Block::Stone),
            (IVec3::new(1, 1, 0), Block::Stone),
            (IVec3::new(0, 1, 1), Block::Stone),
        ]);
        let chunk = chunk_at(IVec3::ZERO);
        let snapshot = ChunkSnapshot::new(&world, chunk);
        let border = BorderBlocks::new(&world, chunk);
        let block_at = |cell: IVec3| {
            if in_chunk(cell) {
                snapshot.block(cell)
            } else {
                border.block(cell)
            }
        };

        assert_eq!(corner_ao(&block_at, IVec3::ZERO, 2), [3, 2, 0, 2]);
    }

    #[test]
    fn a_diagonal_occluder_removes_one_occlusion_level() {
        let world = world_with(&[
            (IVec3::ZERO, Block::Stone),
            (IVec3::new(1, 1, 1), Block::Stone),
        ]);
        let chunk = chunk_at(IVec3::ZERO);
        let snapshot = ChunkSnapshot::new(&world, chunk);
        let border = BorderBlocks::new(&world, chunk);
        let block_at = |cell: IVec3| {
            if in_chunk(cell) {
                snapshot.block(cell)
            } else {
                border.block(cell)
            }
        };

        assert_eq!(corner_ao(&block_at, IVec3::ZERO, 2), [3, 3, 2, 3]);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn greedy_splits_quads_when_ao_differs() {
        let mut flat = World::default();
        let mut occluded = World::default();
        for x in 0..4 {
            flat.set_block(IVec3::new(x, 0, 0), Block::Stone);
            occluded.set_block(IVec3::new(x, 0, 0), Block::Stone);
        }
        occluded.set_block(IVec3::new(0, 1, 1), Block::Stone);

        let top_vertices = |world: &World| {
            mesh(world, chunk_at(IVec3::ZERO), Lod::Lod0)
                .iter()
                .filter(|vertex| vertex.normal == [0.0, 1.0, 0.0] && vertex.position[1] == 1.0)
                .count()
        };

        assert_eq!(top_vertices(&flat), 6);
        assert_eq!(top_vertices(&occluded), 18);
    }

    #[test]
    fn lod_selection_uses_thresholds_and_hysteresis() {
        assert_eq!(select_lod(None, 0.0), Lod::Lod0);
        assert_eq!(select_lod(None, 4.0), Lod::Lod0);
        assert_eq!(select_lod(None, 7.9), Lod::Lod0);
        assert_eq!(select_lod(None, 8.0), Lod::Lod1);
        assert_eq!(select_lod(None, 12.0), Lod::Lod1);

        assert_eq!(select_lod(Some(Lod::Lod0), 4.0), Lod::Lod0);
        assert_eq!(select_lod(Some(Lod::Lod0), 6.0), Lod::Lod0);
        assert_eq!(select_lod(Some(Lod::Lod0), 8.0), Lod::Lod1);

        assert_eq!(select_lod(Some(Lod::Lod1), 8.0), Lod::Lod1);
        assert_eq!(select_lod(Some(Lod::Lod1), 6.0), Lod::Lod1);
        assert_eq!(select_lod(Some(Lod::Lod1), 4.0), Lod::Lod0);
    }

    #[test]
    fn snapshot_and_border_read_the_world() {
        let world = world_with(&[
            (IVec3::new(0, 0, 0), Block::Stone),
            (IVec3::new(-1, 0, 0), Block::Dirt),
            (IVec3::new(32, 0, 0), Block::Grass),
        ]);
        let chunk = chunk_at(IVec3::ZERO);

        let snapshot = ChunkSnapshot::new(&world, chunk);
        assert_eq!(snapshot.block(IVec3::ZERO), Block::Stone);
        assert_eq!(snapshot.block(IVec3::new(-1, 0, 0)), Block::Air);

        let border = BorderBlocks::new(&world, chunk);
        assert_eq!(border.block(IVec3::new(-1, 0, 0)), Block::Dirt);
        assert_eq!(border.block(IVec3::new(32, 0, 0)), Block::Grass);
        assert_eq!(border.block(IVec3::new(0, 32, 0)), Block::Air);
    }

    #[test]
    fn empty_chunk_builds_no_vertices() {
        let world = World::default();

        assert_eq!(mesh(&world, chunk_at(IVec3::ZERO), Lod::Lod0), Vec::new());
        assert_eq!(mesh(&world, chunk_at(IVec3::ZERO), Lod::Lod1), Vec::new());
    }
}
