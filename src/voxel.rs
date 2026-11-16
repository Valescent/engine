//! Chunk mesh generation.
//!
//! A voxel at `(x, y, z)` occupies the unit cube from `(x, y, z)` to
//! `(x + 1, y + 1, z + 1)`. Coordinates are right-handed and Y-up. Meshes are
//! built in chunk-local space; the renderer adds each chunk's origin.

use bytemuck::{Pod, Zeroable};
use glam::{IVec3, Vec3};

use crate::world::{ChunkCoord, World};

/// One mesh vertex: chunk-local position and outward face normal.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct Vertex {
    pub(crate) position: [f32; 3],
    pub(crate) normal: [f32; 3],
}

/// A quad per cube face, wound counter-clockwise from outside the cube.
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
/// coordinate so the output does not depend on `HashSet` iteration order.
pub(crate) fn build_chunk_mesh(world: &World, chunk: ChunkCoord) -> Vec<Vertex> {
    let Some(chunk_data) = world.chunk(chunk) else {
        return Vec::new();
    };

    let mut voxels: Vec<IVec3> = chunk_data.voxels.iter().copied().collect();
    voxels.sort_unstable_by_key(|voxel| (voxel.x, voxel.y, voxel.z));

    let chunk_origin = chunk.world_origin();
    let mut vertices = Vec::with_capacity(voxels.len() * 36);
    for local in voxels {
        push_voxel(world, chunk_origin, local, &mut vertices);
    }
    vertices
}

fn push_voxel(world: &World, chunk_origin: IVec3, local: IVec3, vertices: &mut Vec<Vertex>) {
    let origin = local.as_vec3();
    let world_voxel = chunk_origin + local;

    for (normal, corners) in FACES {
        if checked_add(world_voxel, normal).is_some_and(|neighbour| world.is_solid(neighbour)) {
            continue;
        }

        let points = corners.map(|corner| origin + Vec3::from_array(corner));
        for index in [0, 1, 2, 0, 2, 3] {
            vertices.push(Vertex {
                position: points[index].to_array(),
                normal: normal.as_vec3().to_array(),
            });
        }
    }
}

/// Offsets a world coordinate, returning `None` when the result leaves the
/// `i32` range. Out-of-range neighbours are treated as air.
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

    fn world_with(voxels: &[IVec3]) -> World {
        let mut world = World::default();
        for &voxel in voxels {
            world.insert(voxel);
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
        let mut world = World::default();
        world.insert(IVec3::new(32, 0, 0));

        assert_eq!(mesh(&world, chunk_at(IVec3::ZERO)).len(), 0);
    }

    #[test]
    fn one_voxel_builds_six_faces() {
        let world = world_with(&[IVec3::ZERO]);
        let vertices = mesh(&world, chunk_at(IVec3::ZERO));

        assert_eq!(vertices.len(), 36);
        assert!(vertices.iter().all(|vertex| {
            vertex
                .position
                .iter()
                .all(|coordinate| (0.0..=1.0).contains(coordinate))
        }));
        assert!(
            vertices
                .iter()
                .all(|vertex| vertex.normal.iter().any(|component| *component != 0.0))
        );
    }

    #[test]
    fn two_adjacent_voxels_cull_the_shared_face() {
        let world = world_with(&[IVec3::ZERO, IVec3::X]);

        assert_eq!(mesh(&world, chunk_at(IVec3::ZERO)).len(), 60);
    }

    #[test]
    fn three_voxels_in_a_line_build_eighty_four_vertices() {
        let world = world_with(&[IVec3::ZERO, IVec3::X, IVec3::new(2, 0, 0)]);

        assert_eq!(mesh(&world, chunk_at(IVec3::ZERO)).len(), 84);
    }

    #[test]
    fn faces_across_a_chunk_border_are_culled() {
        let world = world_with(&[IVec3::new(31, 0, 0), IVec3::new(32, 0, 0)]);

        assert_eq!(mesh(&world, chunk_at(IVec3::new(31, 0, 0))).len(), 30);
        assert_eq!(mesh(&world, chunk_at(IVec3::new(32, 0, 0))).len(), 30);
    }

    #[test]
    fn first_face_keeps_the_specified_order_winding_and_normal() {
        let world = world_with(&[IVec3::new(3, 4, 5)]);
        let vertices = mesh(&world, chunk_at(IVec3::new(3, 4, 5)));

        let expected = [
            [4.0, 4.0, 5.0],
            [4.0, 5.0, 5.0],
            [4.0, 5.0, 6.0],
            [4.0, 4.0, 5.0],
            [4.0, 5.0, 6.0],
            [4.0, 4.0, 6.0],
        ];
        for (vertex, position) in vertices.iter().take(6).zip(expected) {
            assert_eq!(vertex.position, position);
            assert_eq!(vertex.normal, [1.0, 0.0, 0.0]);
        }
    }

    #[test]
    fn vertex_positions_stay_chunk_local() {
        let world = world_with(&[IVec3::new(63, 0, 63)]);
        let vertices = mesh(&world, chunk_at(IVec3::new(63, 0, 63)));

        assert!(vertices.iter().all(|vertex| {
            (31.0..=32.0).contains(&vertex.position[0])
                && (0.0..=1.0).contains(&vertex.position[1])
                && (31.0..=32.0).contains(&vertex.position[2])
        }));
    }

    #[test]
    fn world_edges_do_not_wrap_to_the_opposite_side() {
        let world = world_with(&[IVec3::MIN, IVec3::MAX]);

        assert_eq!(mesh(&world, chunk_at(IVec3::MAX)).len(), 36);
        assert_eq!(mesh(&world, chunk_at(IVec3::MIN)).len(), 36);
    }

    #[test]
    fn insertion_order_does_not_change_the_mesh() {
        let forward = world_with(&[IVec3::ZERO, IVec3::X, IVec3::Y, IVec3::Z]);
        let reverse = world_with(&[IVec3::Z, IVec3::Y, IVec3::X, IVec3::ZERO]);

        assert_eq!(
            mesh(&forward, chunk_at(IVec3::ZERO)),
            mesh(&reverse, chunk_at(IVec3::ZERO))
        );
    }

    #[test]
    fn vertex_layout_is_packed() {
        assert_eq!(size_of::<Vertex>(), 24);
    }
}
