//! Voxel storage and mesh generation.
//!
//! A voxel at `(x, y, z)` occupies the unit cube from `(x, y, z)` to
//! `(x + 1, y + 1, z + 1)`. Coordinates are right-handed and Y-up.

use std::collections::HashSet;

use bytemuck::{Pod, Zeroable};
use glam::IVec3;

/// One mesh vertex: world-space position and outward face normal.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct Vertex {
    pub(crate) position: [f32; 3],
    pub(crate) normal: [f32; 3],
}

/// A quad per cube face, wound counter-clockwise from outside the cube.
const FACES: [([f32; 3], [[f32; 3]; 4]); 6] = [
    (
        [1.0, 0.0, 0.0],
        [
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [1.0, 1.0, 1.0],
            [1.0, 0.0, 1.0],
        ],
    ),
    (
        [-1.0, 0.0, 0.0],
        [
            [0.0, 0.0, 1.0],
            [0.0, 1.0, 1.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0],
        ],
    ),
    (
        [0.0, 1.0, 0.0],
        [
            [0.0, 1.0, 0.0],
            [0.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
            [1.0, 1.0, 0.0],
        ],
    ),
    (
        [0.0, -1.0, 0.0],
        [
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
        ],
    ),
    (
        [0.0, 0.0, 1.0],
        [
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
            [0.0, 1.0, 1.0],
            [0.0, 0.0, 1.0],
        ],
    ),
    (
        [0.0, 0.0, -1.0],
        [
            [0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
        ],
    ),
];

/// Builds one non-indexed mesh containing six visible faces per voxel.
///
/// No face culling is performed yet: adjacent voxels still emit their shared
/// faces. Coordinates are sorted so the output does not depend on `HashSet`
/// iteration order.
pub(crate) fn build_mesh(voxels: &HashSet<IVec3>) -> Vec<Vertex> {
    let mut coordinates: Vec<IVec3> = voxels.iter().copied().collect();
    coordinates.sort_unstable_by_key(|voxel| (voxel.x, voxel.y, voxel.z));

    let mut vertices = Vec::with_capacity(coordinates.len() * 36);
    for coordinate in coordinates {
        push_cube(&mut vertices, coordinate);
    }
    vertices
}

fn push_cube(vertices: &mut Vec<Vertex>, coordinate: IVec3) {
    let origin = coordinate.as_vec3();

    for (normal, corners) in FACES {
        let points = corners.map(|corner| {
            [
                origin.x + corner[0],
                origin.y + corner[1],
                origin.z + corner[2],
            ]
        });

        for index in [0, 1, 2, 0, 2, 3] {
            vertices.push(Vertex {
                position: points[index],
                normal,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(coordinates: &[IVec3]) -> HashSet<IVec3> {
        coordinates.iter().copied().collect()
    }

    #[test]
    fn empty_set_builds_no_vertices() {
        assert_eq!(build_mesh(&HashSet::new()).len(), 0);
    }

    #[test]
    fn one_voxel_builds_six_faces() {
        let mesh = build_mesh(&set(&[IVec3::ZERO]));

        assert_eq!(mesh.len(), 36);
        assert!(mesh.iter().all(|vertex| {
            vertex
                .position
                .iter()
                .all(|coordinate| (0.0..=1.0).contains(coordinate))
        }));
        assert!(
            mesh.iter()
                .all(|vertex| vertex.normal.iter().any(|component| *component != 0.0))
        );
    }

    #[test]
    fn adjacent_voxels_are_not_culled() {
        let mesh = build_mesh(&set(&[IVec3::ZERO, IVec3::X]));

        assert_eq!(mesh.len(), 72);
    }

    #[test]
    fn coordinate_order_does_not_change_the_mesh() {
        let forward = build_mesh(&set(&[IVec3::ZERO, IVec3::X, IVec3::Y, IVec3::Z]));
        let reverse = build_mesh(&set(&[IVec3::Z, IVec3::Y, IVec3::X, IVec3::ZERO]));

        assert_eq!(forward, reverse);
    }

    #[test]
    fn vertex_layout_is_packed() {
        assert_eq!(size_of::<Vertex>(), 24);
    }
}
