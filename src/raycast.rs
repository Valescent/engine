//! DDA voxel ray traversal.

use glam::{IVec3, Vec3};

use crate::world::World;

/// The first solid block entered by a ray.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RaycastHit {
    pub(crate) block: IVec3,
    pub(crate) normal: IVec3,
}

/// Walks a ray through the voxel grid and returns the first solid block.
///
/// The start cell counts as already entered; a ray that begins inside a solid
/// block returns `None`. Boundary ties resolve in X, then Y, then Z order.
pub(crate) fn raycast(
    world: &World,
    origin: Vec3,
    direction: Vec3,
    max_distance: f32,
) -> Option<RaycastHit> {
    if max_distance < 0.0 || direction.length_squared() == 0.0 {
        return None;
    }
    let direction = direction.normalize();
    let mut cell = origin.floor().as_ivec3();
    if world.is_solid(cell) {
        return None;
    }

    let step = IVec3::new(
        axis_step(direction.x),
        axis_step(direction.y),
        axis_step(direction.z),
    );
    let mut t_max = Vec3::new(
        initial_t_max(origin.x, direction.x),
        initial_t_max(origin.y, direction.y),
        initial_t_max(origin.z, direction.z),
    );
    let t_delta = Vec3::new(
        axis_delta(direction.x),
        axis_delta(direction.y),
        axis_delta(direction.z),
    );

    for _ in 0..4096 {
        let axis = if t_max.x <= t_max.y && t_max.x <= t_max.z {
            0
        } else if t_max.y <= t_max.z {
            1
        } else {
            2
        };
        let distance = t_max[axis];
        if distance > max_distance {
            return None;
        }

        cell[axis] += step[axis];
        t_max[axis] += t_delta[axis];
        if world.is_solid(cell) {
            let mut normal = IVec3::ZERO;
            normal[axis] = -step[axis];
            return Some(RaycastHit {
                block: cell,
                normal,
            });
        }
    }
    None
}

fn axis_step(direction: f32) -> i32 {
    if direction > 0.0 {
        1
    } else if direction < 0.0 {
        -1
    } else {
        0
    }
}

fn axis_delta(direction: f32) -> f32 {
    if direction == 0.0 {
        f32::INFINITY
    } else {
        direction.abs().recip()
    }
}

fn initial_t_max(origin: f32, direction: f32) -> f32 {
    let boundary = origin.floor();
    if direction > 0.0 {
        (boundary + 1.0 - origin) / direction
    } else if direction < 0.0 {
        (boundary - origin) / direction
    } else {
        f32::INFINITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;

    fn world_with(blocks: &[IVec3]) -> World {
        let mut world = World::default();
        for &block in blocks {
            world.set_block(block, Block::Stone);
        }
        world
    }

    #[test]
    fn hits_a_block_on_the_positive_x_axis() {
        let world = world_with(&[IVec3::new(2, 0, 0)]);
        let hit = raycast(&world, Vec3::new(0.5, 0.5, 0.5), Vec3::X, 6.0).unwrap();

        assert_eq!(hit.block, IVec3::new(2, 0, 0));
        assert_eq!(hit.normal, IVec3::NEG_X);
    }

    #[test]
    fn hits_a_block_on_the_negative_x_axis() {
        let world = world_with(&[IVec3::new(-2, 0, 0)]);
        let hit = raycast(&world, Vec3::new(-0.5, 0.5, 0.5), Vec3::NEG_X, 6.0).unwrap();

        assert_eq!(hit.block, IVec3::new(-2, 0, 0));
        assert_eq!(hit.normal, IVec3::X);
    }

    #[test]
    fn returns_none_for_a_miss() {
        let world = world_with(&[IVec3::new(1, 1, 0)]);

        assert_eq!(
            raycast(&world, Vec3::new(0.5, 0.5, 0.5), Vec3::X, 6.0),
            None
        );
    }

    #[test]
    fn rejects_hits_beyond_the_range() {
        let world = world_with(&[IVec3::new(7, 0, 0)]);

        assert_eq!(
            raycast(&world, Vec3::new(0.5, 0.5, 0.5), Vec3::X, 6.0),
            None
        );
    }

    #[test]
    fn a_ray_starting_inside_a_block_returns_none() {
        let world = world_with(&[IVec3::ZERO]);

        assert_eq!(
            raycast(&world, Vec3::new(0.5, 0.5, 0.5), Vec3::X, 6.0),
            None
        );
    }

    #[test]
    fn a_zero_direction_returns_none() {
        let world = world_with(&[IVec3::new(1, 0, 0)]);

        assert_eq!(
            raycast(&world, Vec3::new(0.5, 0.5, 0.5), Vec3::ZERO, 6.0),
            None
        );
    }

    #[test]
    fn boundary_ties_resolve_x_before_y() {
        let world = world_with(&[IVec3::new(1, 1, 0)]);
        let hit = raycast(
            &world,
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::new(1.0, 1.0, 0.0),
            6.0,
        )
        .unwrap();

        assert_eq!(hit.block, IVec3::new(1, 1, 0));
        assert_eq!(hit.normal, IVec3::NEG_Y);
    }

    #[test]
    fn boundary_ties_resolve_x_before_z() {
        let world = world_with(&[IVec3::new(1, 0, 1)]);
        let hit = raycast(
            &world,
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::new(1.0, 0.0, 1.0),
            6.0,
        )
        .unwrap();

        assert_eq!(hit.block, IVec3::new(1, 0, 1));
        assert_eq!(hit.normal, IVec3::NEG_Z);
    }

    #[test]
    fn diagonal_hits_report_the_entered_face() {
        let world = world_with(&[IVec3::new(2, 1, 0)]);
        let hit = raycast(
            &world,
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::new(1.0, 1.0, 0.0),
            6.0,
        );

        assert!(hit.is_some_and(|hit| hit.block == IVec3::new(2, 1, 0)));
    }
}
