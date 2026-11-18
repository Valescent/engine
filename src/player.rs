//! Walking player physics and swept AABB collision.

use glam::{IVec3, Vec3};

use crate::world::World;

pub(crate) const PLAYER_WIDTH: f32 = 0.6;
pub(crate) const PLAYER_HEIGHT: f32 = 1.8;
pub(crate) const EYE_HEIGHT: f32 = 1.62;
pub(crate) const WALK_SPEED: f32 = 4.5;
pub(crate) const GRAVITY: f32 = 24.0;
pub(crate) const JUMP_VELOCITY: f32 = 8.0;

const HALF_WIDTH: f32 = PLAYER_WIDTH / 2.0;
const COLLISION_EPSILON: f32 = 1e-4;

/// Axis-aligned box in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Aabb {
    pub(crate) min: Vec3,
    pub(crate) max: Vec3,
}

impl Aabb {
    fn from_position(position: Vec3) -> Self {
        Self {
            min: position - Vec3::new(HALF_WIDTH, 0.0, HALF_WIDTH),
            max: position + Vec3::new(HALF_WIDTH, PLAYER_HEIGHT, HALF_WIDTH),
        }
    }

    /// Returns whether this box overlaps a solid unit block.
    pub(crate) fn intersects_block(self, block: IVec3) -> bool {
        let block_min = block.as_vec3();
        let block_max = block_min + Vec3::ONE;
        self.min.x < block_max.x
            && self.max.x > block_min.x
            && self.min.y < block_max.y
            && self.max.y > block_min.y
            && self.min.z < block_max.z
            && self.max.z > block_min.z
    }
}

/// Player physics state. `position` is the centre of the feet.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Player {
    pub(crate) position: Vec3,
    pub(crate) velocity: Vec3,
    pub(crate) on_ground: bool,
}

/// One frame of walking input.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PlayerInput {
    pub(crate) forward: f32,
    pub(crate) right: f32,
    pub(crate) jump: bool,
    pub(crate) yaw: f32,
}

impl Player {
    pub(crate) fn new(position: Vec3) -> Self {
        Self {
            position,
            velocity: Vec3::ZERO,
            on_ground: false,
        }
    }

    pub(crate) fn eye_position(&self) -> Vec3 {
        self.position + Vec3::Y * EYE_HEIGHT
    }

    pub(crate) fn aabb(&self) -> Aabb {
        Aabb::from_position(self.position)
    }

    #[cfg(test)]
    pub(crate) fn update(&mut self, world: &World, input: PlayerInput, delta: f32) {
        self.update_guarded(world, input, delta, &|_| true);
    }

    /// Updates movement, rejecting any axis that enters an unready column.
    pub(crate) fn update_guarded(
        &mut self,
        world: &World,
        input: PlayerInput,
        delta: f32,
        ready: &impl Fn(Vec3) -> bool,
    ) {
        if delta <= 0.0 {
            return;
        }

        let (yaw_sin, yaw_cos) = input.yaw.sin_cos();
        let forward = Vec3::new(yaw_sin, 0.0, -yaw_cos);
        let right = Vec3::new(yaw_cos, 0.0, yaw_sin);
        let direction = (forward * input.forward + right * input.right).normalize_or_zero();
        self.velocity.x = direction.x * WALK_SPEED;
        self.velocity.z = direction.z * WALK_SPEED;

        if input.jump && self.on_ground {
            self.velocity.y = JUMP_VELOCITY;
        }
        self.velocity.y -= GRAVITY * delta;

        for (axis, distance) in [
            (1, self.velocity.y * delta),
            (0, self.velocity.x * delta),
            (2, self.velocity.z * delta),
        ] {
            let before = self.position;
            self.move_axis(world, axis, distance);
            if !ready(self.position) {
                self.position = before;
                self.velocity[axis] = 0.0;
            }
        }
    }

    fn move_axis(&mut self, world: &World, axis: usize, distance: f32) {
        if distance == 0.0 {
            return;
        }

        let current = self.aabb();
        let mut swept = current;
        swept.min[axis] = current.min[axis].min(current.min[axis] + distance);
        swept.max[axis] = current.max[axis].max(current.max[axis] + distance);

        let min = (swept.min - Vec3::splat(COLLISION_EPSILON))
            .floor()
            .as_ivec3();
        let max = swept.max.floor().as_ivec3();
        let mut best = distance;
        let mut hit = false;

        for block in world.solid_blocks_in(min, max) {
            if !overlaps_other_axes(current, block, axis) {
                continue;
            }

            let block_min = block.as_vec3();
            let block_max = block_min + Vec3::ONE;
            if distance > 0.0 {
                let gap = block_min[axis] - current.max[axis];
                if gap >= -COLLISION_EPSILON && gap < best {
                    best = gap.max(0.0);
                    hit = true;
                }
            } else {
                let gap = block_max[axis] - current.min[axis];
                if gap <= COLLISION_EPSILON && gap > best {
                    best = gap.min(0.0);
                    hit = true;
                }
            }
        }

        self.position[axis] += best;
        if hit {
            self.velocity[axis] = 0.0;
            if axis == 1 {
                self.on_ground = distance < 0.0;
            }
        } else if axis == 1 {
            self.on_ground = false;
        }
    }
}

fn overlaps_other_axes(aabb: Aabb, block: IVec3, axis: usize) -> bool {
    let block_min = block.as_vec3();
    let block_max = block_min + Vec3::ONE;
    for other in 0..3 {
        if other == axis {
            continue;
        }
        if aabb.max[other] <= block_min[other] + COLLISION_EPSILON
            || aabb.min[other] >= block_max[other] - COLLISION_EPSILON
        {
            return false;
        }
    }
    true
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

    fn no_input(jump: bool) -> PlayerInput {
        PlayerInput {
            forward: 0.0,
            right: 0.0,
            jump,
            yaw: 0.0,
        }
    }

    #[test]
    fn aabb_and_eye_match_the_spec() {
        let player = Player::new(Vec3::new(2.0, 3.0, 4.0));
        let aabb = player.aabb();

        assert_eq!(aabb.min, Vec3::new(1.7, 3.0, 3.7));
        assert_eq!(aabb.max, Vec3::new(2.3, 4.8, 4.3));
        assert_eq!(player.eye_position(), Vec3::new(2.0, 4.62, 4.0));
    }

    #[test]
    fn zero_delta_is_a_no_op() {
        let world = World::default();
        let mut player = Player::new(Vec3::new(0.5, 2.0, 0.5));

        player.update(&world, no_input(false), 0.0);

        assert_eq!(player.position, Vec3::new(0.5, 2.0, 0.5));
        assert_eq!(player.velocity, Vec3::ZERO);
    }

    #[test]
    fn gravity_accelerates_downward() {
        let world = World::default();
        let mut player = Player::new(Vec3::new(0.5, 5.0, 0.5));

        player.update(&world, no_input(false), 0.1);

        assert!((player.velocity.y + 2.4).abs() < 1e-5);
        assert!((player.position.y - 4.76).abs() < 1e-5);
        assert!(!player.on_ground);
    }

    #[test]
    fn lands_on_the_floor_and_stops_falling() {
        let world = world_with(&[
            IVec3::new(0, 0, 0),
            IVec3::new(1, 0, 0),
            IVec3::new(0, 0, 1),
            IVec3::new(1, 0, 1),
        ]);
        let mut player = Player::new(Vec3::new(0.5, 3.0, 0.5));

        for _ in 0..120 {
            player.update(&world, no_input(false), 0.1);
            if player.on_ground {
                break;
            }
        }

        assert!(player.on_ground);
        assert!((player.position.y - 1.0).abs() < 1e-4);
        assert_eq!(player.velocity.y, 0.0);
    }

    #[test]
    fn jumping_requires_ground_and_raises_the_player() {
        let world = world_with(&[IVec3::new(0, 0, 0)]);
        let mut player = Player::new(Vec3::new(0.5, 1.0, 0.5));
        player.on_ground = true;

        player.update(&world, no_input(true), 0.01);

        assert!(player.position.y > 1.0);
        assert!(player.velocity.y > 0.0);
        assert!(!player.on_ground);
    }

    #[test]
    fn air_jumps_are_ignored() {
        let world = World::default();
        let mut player = Player::new(Vec3::new(0.5, 5.0, 0.5));
        player.on_ground = false;

        player.update(&world, no_input(true), 0.1);

        assert!(player.velocity.y < 0.0);
    }

    #[test]
    fn walking_uses_yaw_and_ignores_vertical_look() {
        let world = World::default();
        let mut player = Player::new(Vec3::new(0.0, 5.0, 0.0));

        player.update(
            &world,
            PlayerInput {
                forward: 1.0,
                right: 0.0,
                jump: false,
                yaw: std::f32::consts::FRAC_PI_2,
            },
            1.0,
        );

        assert!((player.position.x - 4.5).abs() < 1e-5);
        assert!(player.position.z.abs() < 1e-5);
    }

    #[test]
    fn wall_collision_stops_at_the_contact_plane_without_step_up() {
        let world = world_with(&[
            IVec3::new(0, 0, 0),
            IVec3::new(1, 0, 0),
            IVec3::new(1, 1, 0),
        ]);
        let mut player = Player::new(Vec3::new(0.5, 1.0, 0.5));
        player.on_ground = true;

        player.update(
            &world,
            PlayerInput {
                forward: 1.0,
                right: 0.0,
                jump: false,
                yaw: std::f32::consts::FRAC_PI_2,
            },
            1.0,
        );

        assert!((player.position.x - 0.7).abs() < 1e-4);
        assert!((player.position.y - 1.0).abs() < 1e-4);
        assert_eq!(player.velocity.x, 0.0);
    }

    #[test]
    fn fast_falls_do_not_tunnel_through_the_floor() {
        let world = world_with(&[IVec3::new(0, 0, 0), IVec3::new(1, 0, 0)]);
        let mut player = Player::new(Vec3::new(0.5, 20.0, 0.5));
        player.velocity.y = -500.0;

        player.update(&world, no_input(false), 0.1);

        assert!((player.position.y - 1.0).abs() < 1e-4);
        assert!(player.on_ground);
    }

    #[test]
    fn diagonal_movement_stops_on_both_axes() {
        let world = world_with(&[
            IVec3::new(0, 0, 0),
            IVec3::new(1, 0, 0),
            IVec3::new(0, 0, 1),
            IVec3::new(1, 1, 0),
            IVec3::new(0, 1, 1),
        ]);
        let mut player = Player::new(Vec3::new(0.5, 1.0, 0.5));
        player.on_ground = true;

        player.update(
            &world,
            PlayerInput {
                forward: 1.0,
                right: 1.0,
                jump: false,
                yaw: std::f32::consts::FRAC_PI_2,
            },
            1.0,
        );

        assert!(player.position.x < 0.75);
        assert!(player.position.z < 0.75);
        assert_eq!(player.velocity.x, 0.0);
        assert_eq!(player.velocity.z, 0.0);
        assert!(!player.aabb().intersects_block(IVec3::new(1, 1, 0)));
    }
}
