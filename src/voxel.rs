//! Highlight mesh and the v0.0.4 face-culled mesher kept as a test reference.
//!
//! A voxel at `(x, y, z)` occupies the unit cube from `(x, y, z)` to
//! `(x + 1, y + 1, z + 1)`. Coordinates are right-handed and Y-up. Chunk meshes
//! are built in chunk-local space; the renderer adds each chunk's origin.

use bytemuck::{Pod, Zeroable};
use glam::{IVec3, Vec3};

/// One mesh vertex: packed chunk-local cell, face, texture UV, array layer, and
/// packed light/AO.
///
/// Packing keeps the vertex at 12 bytes: three cell bytes plus the face index,
/// then two UV bytes, the texture layer, and a reserved byte, then the light/AO
/// word. `position`, `normal`, `uv`, and `layer` unpack for tests and tooling.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Pod, Zeroable)]
pub(crate) struct Vertex {
    pub(crate) cell: [u8; 3],
    pub(crate) face: u8,
    pub(crate) texel: [u8; 2],
    pub(crate) texture_layer: u8,
    pub(crate) reserved: u8,
    pub(crate) light_ao: u32,
}

/// Face normals in the mesher's face order: +X, -X, +Y, -Y, +Z, -Z.
const FACE_DIRECTIONS: [[f32; 3]; 6] = [
    [1.0, 0.0, 0.0],
    [-1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, -1.0, 0.0],
    [0.0, 0.0, 1.0],
    [0.0, 0.0, -1.0],
];

impl Vertex {
    /// Packs one face vertex. Positions and UVs must be whole blocks in `0..=255`.
    pub(crate) fn new(
        position: [f32; 3],
        normal: [f32; 3],
        uv: [f32; 2],
        layer: u32,
        light_ao: u32,
    ) -> Self {
        Self {
            cell: position.map(pack_byte),
            face: face_index(normal),
            texel: uv.map(pack_byte),
            texture_layer: u8::try_from(layer).unwrap_or(u8::MAX),
            reserved: 0,
            light_ao,
        }
    }

    #[cfg(test)]
    pub(crate) fn position(&self) -> [f32; 3] {
        self.cell.map(f32::from)
    }

    #[cfg(test)]
    pub(crate) fn normal(&self) -> [f32; 3] {
        FACE_DIRECTIONS[usize::from(self.face)]
    }

    #[cfg(test)]
    pub(crate) fn uv(&self) -> [f32; 2] {
        self.texel.map(f32::from)
    }

    #[cfg(test)]
    pub(crate) fn layer(&self) -> u32 {
        u32::from(self.texture_layer)
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn pack_byte(value: f32) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

#[allow(clippy::float_cmp)]
fn face_index(normal: [f32; 3]) -> u8 {
    FACE_DIRECTIONS
        .iter()
        .position(|face| face[0] == normal[0] && face[1] == normal[1] && face[2] == normal[2])
        .and_then(|index| u8::try_from(index).ok())
        .unwrap_or(0)
}

/// One wireframe vertex in world space.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct HighlightVertex {
    pub(crate) position: [f32; 3],
}

/// The quad UVs in face-corner order.
#[cfg(test)]
const UVS: [[f32; 2]; 4] = [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]];

/// A quad per cube face, wound counter-clockwise from outside the cube.
#[cfg(test)]
const FACES: [(IVec3, [[f32; 3]; 4]); 6] = [
    (
        IVec3::X,
        [
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [1.0, 1.0, 1.0],
            [1.0, 0.0, 1.0],
        ],
    ),
    (
        IVec3::NEG_X,
        [
            [0.0, 0.0, 1.0],
            [0.0, 1.0, 1.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0],
        ],
    ),
    (
        IVec3::Y,
        [
            [0.0, 1.0, 0.0],
            [0.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
            [1.0, 1.0, 0.0],
        ],
    ),
    (
        IVec3::NEG_Y,
        [
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
        ],
    ),
    (
        IVec3::Z,
        [
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
            [0.0, 1.0, 1.0],
            [0.0, 0.0, 1.0],
        ],
    ),
    (
        IVec3::NEG_Z,
        [
            [0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
        ],
    ),
];

/// Builds one non-indexed mesh of the faces exposed to air.
///
/// The chunk origin plus local coordinates maps every face neighbour into world
/// space, so faces shared across a chunk border are culled like any other.
/// Neighbours beyond the `i32` world bounds are air. Voxels are sorted by local
/// coordinate so the output does not depend on `HashMap` iteration order.
#[cfg(test)]
pub(crate) fn build_chunk_mesh(
    world: &crate::world::World,
    chunk: crate::world::ChunkCoord,
) -> Vec<Vertex> {
    let Some(chunk_data) = world.chunk(chunk) else {
        return Vec::new();
    };

    let mut voxels: Vec<IVec3> = chunk_data.iter().map(|(local, _)| local).collect();
    voxels.sort_unstable_by_key(|voxel| (voxel.x, voxel.y, voxel.z));

    let chunk_origin = chunk.world_origin();
    let mut vertices = Vec::with_capacity(voxels.len() * 36);
    for local in voxels {
        let block = chunk_data.block(local);
        push_voxel(world, chunk_origin, local, block, &mut vertices);
    }
    vertices
}

#[cfg(test)]
fn push_voxel(
    world: &crate::world::World,
    chunk_origin: IVec3,
    local: IVec3,
    block: crate::block::Block,
    vertices: &mut Vec<Vertex>,
) {
    let Some(layers) = block.layers() else {
        return;
    };
    let origin = local.as_vec3();
    let world_voxel = chunk_origin + local;

    for (normal, corners) in FACES {
        if checked_add(world_voxel, normal).is_some_and(|neighbour| world.is_solid(neighbour)) {
            continue;
        }

        let layer = match normal.y {
            1 => layers.top,
            -1 => layers.bottom,
            _ => layers.side,
        };
        let points = corners.map(|corner| origin + Vec3::from_array(corner));
        for index in [0, 1, 2, 0, 2, 3] {
            vertices.push(Vertex::new(
                points[index].to_array(),
                normal.as_vec3().to_array(),
                UVS[index],
                layer,
                0,
            ));
        }
    }
}

/// Builds the 24-vertex line list for a block outline in world space.
pub(crate) fn build_highlight_mesh(block: IVec3) -> Vec<HighlightVertex> {
    const EXPANSION: f32 = 0.002;
    const EDGES: [(usize, usize); 12] = [
        (0, 1),
        (1, 2),
        (2, 3),
        (3, 0),
        (4, 5),
        (5, 6),
        (6, 7),
        (7, 4),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ];

    let min = block.as_vec3() - Vec3::splat(EXPANSION);
    let max = block.as_vec3() + Vec3::ONE + Vec3::splat(EXPANSION);
    let corners = [
        Vec3::new(min.x, min.y, min.z),
        Vec3::new(max.x, min.y, min.z),
        Vec3::new(max.x, max.y, min.z),
        Vec3::new(min.x, max.y, min.z),
        Vec3::new(min.x, min.y, max.z),
        Vec3::new(max.x, min.y, max.z),
        Vec3::new(max.x, max.y, max.z),
        Vec3::new(min.x, max.y, max.z),
    ];

    let mut vertices = Vec::with_capacity(EDGES.len() * 2);
    for (start, end) in EDGES {
        vertices.push(HighlightVertex {
            position: corners[start].to_array(),
        });
        vertices.push(HighlightVertex {
            position: corners[end].to_array(),
        });
    }
    vertices
}

/// Offsets a world coordinate, returning `None` when the result leaves the
/// `i32` range. Out-of-range neighbours are treated as air.
#[cfg(test)]
fn checked_add(position: IVec3, offset: IVec3) -> Option<IVec3> {
    Some(IVec3::new(
        position.x.checked_add(offset.x)?,
        position.y.checked_add(offset.y)?,
        position.z.checked_add(offset.z)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;
    use crate::world::{ChunkCoord, World};

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

    fn mesh(world: &World, chunk: ChunkCoord) -> Vec<Vertex> {
        build_chunk_mesh(world, chunk)
    }

    #[test]
    fn missing_chunk_builds_no_vertices() {
        let world = World::default();

        assert_eq!(mesh(&world, chunk_at(IVec3::ZERO)).len(), 0);
    }

    #[test]
    fn empty_chunk_builds_no_vertices() {
        let world = world_with(&[(IVec3::new(32, 0, 0), Block::Stone)]);

        assert_eq!(mesh(&world, chunk_at(IVec3::ZERO)).len(), 0);
    }

    #[test]
    fn one_voxel_builds_six_faces() {
        let world = world_with(&[(IVec3::ZERO, Block::Stone)]);
        let vertices = mesh(&world, chunk_at(IVec3::ZERO));

        assert_eq!(vertices.len(), 36);
        assert!(vertices.iter().all(|vertex| {
            vertex
                .position()
                .iter()
                .all(|coordinate| (0.0..=1.0).contains(coordinate))
        }));
        assert!(
            vertices
                .iter()
                .all(|vertex| vertex.normal().iter().any(|component| *component != 0.0))
        );
    }

    #[test]
    fn two_adjacent_voxels_cull_the_shared_face() {
        let world = world_with(&[(IVec3::ZERO, Block::Stone), (IVec3::X, Block::Dirt)]);

        assert_eq!(mesh(&world, chunk_at(IVec3::ZERO)).len(), 60);
    }

    #[test]
    fn three_voxels_in_a_line_build_eighty_four_vertices() {
        let world = world_with(&[
            (IVec3::ZERO, Block::Stone),
            (IVec3::X, Block::Stone),
            (IVec3::new(2, 0, 0), Block::Stone),
        ]);

        assert_eq!(mesh(&world, chunk_at(IVec3::ZERO)).len(), 84);
    }

    #[test]
    fn faces_across_a_chunk_border_are_culled() {
        let world = world_with(&[
            (IVec3::new(31, 0, 0), Block::Stone),
            (IVec3::new(32, 0, 0), Block::Stone),
        ]);

        assert_eq!(mesh(&world, chunk_at(IVec3::new(31, 0, 0))).len(), 30);
        assert_eq!(mesh(&world, chunk_at(IVec3::new(32, 0, 0))).len(), 30);
    }

    #[test]
    fn first_face_keeps_the_specified_order_winding_normal_and_uvs() {
        let world = world_with(&[(IVec3::new(3, 4, 5), Block::Stone)]);
        let vertices = mesh(&world, chunk_at(IVec3::new(3, 4, 5)));

        let expected_positions = [
            [4.0, 4.0, 5.0],
            [4.0, 5.0, 5.0],
            [4.0, 5.0, 6.0],
            [4.0, 4.0, 5.0],
            [4.0, 5.0, 6.0],
            [4.0, 4.0, 6.0],
        ];
        let expected_uvs = [
            [0.0, 0.0],
            [0.0, 1.0],
            [1.0, 1.0],
            [0.0, 0.0],
            [1.0, 1.0],
            [1.0, 0.0],
        ];
        for ((vertex, position), uv) in vertices
            .iter()
            .take(6)
            .zip(expected_positions)
            .zip(expected_uvs)
        {
            assert_eq!(vertex.position(), position);
            assert_eq!(vertex.normal(), [1.0, 0.0, 0.0]);
            assert_eq!(vertex.uv(), uv);
            assert_eq!(vertex.layer(), 0);
        }
    }

    #[test]
    fn faces_use_their_block_face_layer() {
        let world = world_with(&[(IVec3::ZERO, Block::Grass)]);
        let vertices = mesh(&world, chunk_at(IVec3::ZERO));

        for vertex in &vertices {
            let expected = if vertex.normal()[1] > 0.5 {
                2
            } else if vertex.normal()[1] < -0.5 {
                4
            } else {
                3
            };
            assert_eq!(vertex.layer(), expected, "{:?}", vertex.normal());
        }

        let wood = world_with(&[(IVec3::ZERO, Block::Wood)]);
        let vertices = mesh(&wood, chunk_at(IVec3::ZERO));
        assert!(vertices.iter().all(|vertex| {
            let expected = if vertex.normal()[1].abs() > 0.5 { 5 } else { 6 };
            vertex.layer() == expected
        }));
    }

    #[test]
    fn vertex_positions_stay_chunk_local() {
        let world = world_with(&[(IVec3::new(63, 0, 63), Block::Stone)]);
        let vertices = mesh(&world, chunk_at(IVec3::new(63, 0, 63)));

        assert!(vertices.iter().all(|vertex| {
            (31.0..=32.0).contains(&vertex.position()[0])
                && (0.0..=1.0).contains(&vertex.position()[1])
                && (31.0..=32.0).contains(&vertex.position()[2])
        }));
    }

    #[test]
    fn world_edges_do_not_wrap_to_the_opposite_side() {
        let world = world_with(&[(IVec3::MIN, Block::Stone), (IVec3::MAX, Block::Stone)]);

        assert_eq!(mesh(&world, chunk_at(IVec3::MAX)).len(), 36);
        assert_eq!(mesh(&world, chunk_at(IVec3::MIN)).len(), 36);
    }

    #[test]
    fn insertion_order_does_not_change_the_mesh() {
        let forward = world_with(&[
            (IVec3::ZERO, Block::Stone),
            (IVec3::X, Block::Dirt),
            (IVec3::Y, Block::Grass),
            (IVec3::Z, Block::Wood),
        ]);
        let reverse = world_with(&[
            (IVec3::Z, Block::Wood),
            (IVec3::Y, Block::Grass),
            (IVec3::X, Block::Dirt),
            (IVec3::ZERO, Block::Stone),
        ]);

        assert_eq!(
            mesh(&forward, chunk_at(IVec3::ZERO)),
            mesh(&reverse, chunk_at(IVec3::ZERO))
        );
    }

    #[test]
    fn vertex_layout_is_packed() {
        assert_eq!(size_of::<Vertex>(), 12);
        assert_eq!(size_of::<HighlightVertex>(), 12);
    }

    #[test]
    fn highlight_has_twelve_edges_and_surrounds_the_block() {
        let mesh = build_highlight_mesh(IVec3::new(2, 3, 4));

        assert_eq!(mesh.len(), 24);
        assert!(mesh.iter().all(|vertex| {
            let position = Vec3::from_array(vertex.position);
            position.cmpge(Vec3::new(1.998, 2.998, 3.998)).all()
                && position.cmple(Vec3::new(3.002, 4.002, 5.002)).all()
        }));
    }
}
