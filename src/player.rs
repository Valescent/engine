//! Walking player physics and swept AABB collision.

use glam::{IVec3, Vec3};

pub(crate) const PLAYER_WIDTH: f32 = 0.6;
pub(crate) const PLAYER_HEIGHT: f32 = 1.8;
pub(crate) const EYE_HEIGHT: f32 = 1.62;
pub(crate) const WALK_SPEED: f32 = 4.5;
/// Sprinting speed while stamina remains.
pub(crate) const SPRINT_SPEED: f32 = 6.3;
pub(crate) const GRAVITY: f32 = 24.0;
pub(crate) const JUMP_VELOCITY: f32 = 8.0;
/// Horizontal and vertical speed while the debug fly toggle is on.
pub(crate) const FLY_SPEED: f32 = 12.0;

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

/// The block queries movement needs.
///
/// Implemented by the legacy [`World`] and by the new-terrain collision index, so
/// the player does not care which world is live (`F1.5` keeps one baseline).
pub(crate) trait BlockSource {
    fn solid_blocks_in(&self, min: IVec3, max: IVec3) -> Vec<IVec3>;
}

/// Player physics state. `position` is the centre of the feet.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Player {
    pub(crate) position: Vec3,
    pub(crate) velocity: Vec3,
    pub(crate) on_ground: bool,
    /// Debug fly: no gravity, no collision, `Space` up and `Ctrl` down.
    pub(crate) flying: bool,
    fall_peak: f32,
    fall_distance: f32,
}

/// One frame of walking input.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PlayerInput {
    pub(crate) forward: f32,
    pub(crate) right: f32,
    pub(crate) jump: bool,
    /// `Ctrl` while flying: descend. Ignored on foot.
    pub(crate) descend: bool,
    /// Request the sprint speed multiplier.
    pub(crate) sprint: bool,
    pub(crate) yaw: f32,
}

impl Player {
    pub(crate) fn new(position: Vec3) -> Self {
        Self {
            position,
            velocity: Vec3::ZERO,
            on_ground: false,
            flying: false,
            fall_peak: position.y,
            fall_distance: 0.0,
        }
    }

    /// Resets motion and fall tracking at a respawn point.
    pub(crate) fn respawn(&mut self, position: Vec3) {
        self.position = position;
        self.velocity = Vec3::ZERO;
        self.on_ground = false;
        self.flying = false;
        self.fall_peak = position.y;
        self.fall_distance = 0.0;
    }

    /// Takes the distance of the last completed fall, if any.
    pub(crate) fn take_fall_distance(&mut self) -> f32 {
        std::mem::take(&mut self.fall_distance)
    }

    /// Toggles debug flight. Leaving it restores gravity and collision; entering
    /// it clears vertical motion so a frame cannot apply both.
    pub(crate) fn toggle_fly(&mut self) {
        self.flying = !self.flying;
        self.velocity = Vec3::ZERO;
        self.on_ground = false;
        self.fall_distance = 0.0;
        self.fall_peak = self.position.y;
    }

    pub(crate) fn eye_position(&self) -> Vec3 {
        self.position + Vec3::Y * EYE_HEIGHT
    }

    pub(crate) fn aabb(&self) -> Aabb {
        Aabb::from_position(self.position)
    }

    #[cfg(test)]
    pub(crate) fn update(&mut self, world: &dyn BlockSource, input: PlayerInput, delta: f32) {
        self.update_guarded(world, input, delta, &|_| true);
    }

    /// Updates movement, rejecting any axis that enters an unready column.
    pub(crate) fn update_guarded(
        &mut self,
        world: &dyn BlockSource,
        input: PlayerInput,
        delta: f32,
        ready: &impl Fn(Vec3) -> bool,
    ) {
        if delta <= 0.0 {
            return;
        }

        let was_on_ground = self.on_ground;
        let (yaw_sin, yaw_cos) = input.yaw.sin_cos();
        let forward = Vec3::new(yaw_sin, 0.0, -yaw_cos);
        let right = Vec3::new(yaw_cos, 0.0, yaw_sin);
        let direction = (forward * input.forward + right * input.right).normalize_or_zero();
        let speed = if self.flying {
            FLY_SPEED
        } else if input.sprint && direction.length_squared() > 0.0 {
            SPRINT_SPEED
        } else {
            WALK_SPEED
        };
        self.velocity.x = direction.x * speed;
        self.velocity.z = direction.z * speed;

        if self.flying {
            // Debug flight: no gravity, no collision, no ready-column guard.
            let up = if input.jump { 1.0 } else { 0.0 };
            let down = if input.descend { 1.0 } else { 0.0 };
            self.velocity.y = (up - down) * FLY_SPEED;
            self.position += self.velocity * delta;
            self.on_ground = false;
            self.fall_distance = 0.0;
            self.fall_peak = self.position.y;
            return;
        }

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

        if self.on_ground {
            if !was_on_ground {
                self.fall_distance = (self.fall_peak - self.position.y).max(0.0);
            }
            self.fall_peak = self.position.y;
        } else {
            self.fall_peak = self.fall_peak.max(self.position.y);
        }
    }

    fn move_axis(&mut self, world: &dyn BlockSource, axis: usize, distance: f32) {
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
    use crate::world::World;

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
            descend: false,
            sprint: false,
            yaw: 0.0,
        }
    }

    #[test]
    fn flying_ignores_gravity_and_moves_vertically() {
        let mut player = Player::new(Vec3::new(16.0, 200.0, 16.0));
        player.toggle_fly();
        assert!(player.flying);

        // Up: `jump` rises at FLY_SPEED, with no gravity term.
        player.update(&World::default(), no_input(true), 0.5);
        assert!(
            (player.position.y - 206.0).abs() < 1e-4,
            "{:?}",
            player.position.y
        );

        // Down: `descend` falls at the same speed.
        let mut down = no_input(false);
        down.descend = true;
        player.update(&World::default(), down, 0.5);
        assert!(
            (player.position.y - 200.0).abs() < 1e-4,
            "{:?}",
            player.position.y
        );

        // Neither key: hover, no gravity.
        player.update(&World::default(), no_input(false), 1.0);
        assert!((player.position.y - 200.0).abs() < 1e-4);

        // Leaving flight restores gravity on the next frame.
        player.toggle_fly();
        assert!(!player.flying);
        player.update(&World::default(), no_input(false), 0.5);
        assert!(player.position.y < 200.0);
    }

    #[test]
    fn the_guard_stops_movement_at_an_unready_column() {
        let mut world = World::default();
        for x in 0..64 {
            for z in 0..64 {
                world.set_block(IVec3::new(x, 0, z), Block::Stone);
            }
        }
        let mut player = Player::new(Vec3::new(30.0, 1.0, 16.5));
        let input = PlayerInput {
            forward: 0.0,
            right: 1.0,
            jump: false,
            descend: false,
            sprint: false,
            yaw: 0.0,
        };
        let ready = |position: Vec3| position.x < 31.0;

        for _ in 0..240 {
            player.update_guarded(&world, input, 1.0 / 60.0, &ready);
        }

        assert!(player.position.x < 31.0, "{:?}", player.position);
        assert_eq!(player.velocity.x, 0.0);
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
                descend: false,
                sprint: false,
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
                descend: false,
                sprint: false,
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
                descend: false,
                sprint: false,
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

    #[test]
    fn sprinting_moves_faster_than_walking() {
        let world = world_with(&[]);
        let mut walking = Player::new(Vec3::ZERO);
        let mut sprinting = Player::new(Vec3::ZERO);
        let mut walk_input = no_input(false);
        walk_input.forward = 1.0;
        let mut sprint_input = walk_input;
        sprint_input.sprint = true;

        walking.update(&world, walk_input, 0.1);
        sprinting.update(&world, sprint_input, 0.1);

        assert!(sprinting.position.z.abs() > walking.position.z.abs());
    }

    #[test]
    fn a_landing_reports_the_fall_distance() {
        let world = world_with(&[IVec3::new(0, 0, 0)]);
        let mut player = Player::new(Vec3::new(0.5, 5.0, 0.5));

        for _ in 0..240 {
            player.update(&world, no_input(false), 1.0 / 60.0);
        }

        assert!(player.on_ground);
        assert!(player.take_fall_distance() > 3.0);
    }
}
