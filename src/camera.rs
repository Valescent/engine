//! First-person camera state and look direction.

use std::f32::consts::{PI, TAU};

use glam::camera::rh::{proj::directx, view::look_to_mat4};
use glam::{IVec3, Mat4, Vec3};

const MAX_PITCH_RADIANS: f32 = 89.0 * PI / 180.0;

/// One frame's camera-relative render origin and view transform.
///
/// `origin` is the integral world block that every submitted section/chunk
/// origin is measured from, and `view_projection` already includes
/// `camera_position - origin`. The CPU world model stays in world coordinates;
/// only the draw inputs are rebased.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RenderView {
    /// World block at relative `(0, 0, 0)` for this frame.
    pub(crate) origin: IVec3,
    /// Camera-relative view-projection (DirectX/WebGPU depth range).
    pub(crate) view_projection: Mat4,
    /// Camera position minus `origin`, used for fog distance.
    pub(crate) camera_position: Vec3,
    /// Camera far plane in blocks, mirrored into the frame uniform.
    pub(crate) far_plane: f32,
    /// Distance where fog is fully opaque, mirrored into the frame uniform.
    pub(crate) fog_end: f32,
}

impl RenderView {
    /// Converts a world-space point into the frame's relative space.
    #[cfg(test)]
    pub(crate) fn relative_point(self, world: Vec3) -> Vec3 {
        world - self.origin.as_vec3()
    }

    /// Converts a world-space block origin into the frame's relative space.
    #[cfg(test)]
    pub(crate) fn relative_block(self, world: [i32; 3]) -> Vec3 {
        relative_origin(world, self.origin)
    }
}

/// Subtracts the render origin from an integer block origin.
///
/// The difference is taken in `i64` and only then rounded to `f32`, so a
/// section or chunk near the camera keeps block precision even when the world
/// coordinate itself is far outside `f32`'s exact-integer range.
#[allow(
    clippy::cast_precision_loss,
    reason = "a section within the view distance is far below 2^24 blocks from the camera"
)]
pub(crate) fn relative_origin(world: [i32; 3], origin: IVec3) -> Vec3 {
    Vec3::new(
        (i64::from(world[0]) - i64::from(origin.x)) as f32,
        (i64::from(world[1]) - i64::from(origin.y)) as f32,
        (i64::from(world[2]) - i64::from(origin.z)) as f32,
    )
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Camera {
    position: Vec3,
    yaw: f32,
    pitch: f32,
    aspect: f32,
    fov_y_radians: f32,
    z_near: f32,
    z_far: f32,
    fog_end: f32,
    sensitivity: f32,
}

impl Camera {
    pub(crate) fn new(aspect: f32) -> Self {
        Self {
            position: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            aspect,
            fov_y_radians: std::f32::consts::FRAC_PI_3,
            z_near: 0.1,
            z_far: 512.0,
            fog_end: 384.0,
            sensitivity: 0.0025,
        }
    }

    pub(crate) fn set_aspect(&mut self, width: u32, height: u32) {
        self.aspect = aspect_ratio(width, height);
    }

    pub(crate) fn set_position(&mut self, position: Vec3) {
        self.position = position;
    }

    #[cfg(test)]
    pub(crate) fn position(&self) -> Vec3 {
        self.position
    }

    pub(crate) fn far_plane(&self) -> f32 {
        self.z_far
    }

    pub(crate) fn set_far_plane(&mut self, far_plane: f32) {
        self.z_far = far_plane;
    }

    /// Distance where fog is fully opaque.
    pub(crate) fn fog_end(&self) -> f32 {
        self.fog_end
    }

    pub(crate) fn set_fog_end(&mut self, fog_end: f32) {
        self.fog_end = fog_end;
    }

    pub(crate) fn yaw(&self) -> f32 {
        self.yaw
    }

    pub(crate) fn set_yaw(&mut self, yaw: f32) {
        self.yaw = wrap_angle(yaw);
    }

    pub(crate) fn look(&mut self, delta_x: f32, delta_y: f32) {
        self.yaw = wrap_angle(self.yaw + delta_x * self.sensitivity);
        self.pitch =
            (self.pitch - delta_y * self.sensitivity).clamp(-MAX_PITCH_RADIANS, MAX_PITCH_RADIANS);
    }

    /// Builds this frame's camera-relative view origin and view-projection.
    ///
    /// The origin is the camera's block position. Moving the camera by whole
    /// blocks therefore leaves the matrix unchanged; only the submitted
    /// section/chunk origins move by the same whole-block amount.
    pub(crate) fn render_view(&self) -> RenderView {
        let origin = self.position.floor().as_ivec3();
        let camera_position = self.position - origin.as_vec3();
        RenderView {
            origin,
            view_projection: directx::perspective(
                self.fov_y_radians,
                self.aspect,
                self.z_near,
                self.z_far,
            ) * look_to_mat4(camera_position, self.forward(), Vec3::Y),
            camera_position,
            far_plane: self.z_far,
            fog_end: self.fog_end,
        }
    }

    #[cfg(test)]
    pub(crate) fn view_projection(&self) -> Mat4 {
        self.render_view().view_projection
    }

    pub(crate) fn forward(&self) -> Vec3 {
        let (yaw_sin, yaw_cos) = self.yaw.sin_cos();
        let (pitch_sin, pitch_cos) = self.pitch.sin_cos();
        Vec3::new(yaw_sin * pitch_cos, pitch_sin, -yaw_cos * pitch_cos)
    }
}

#[allow(clippy::cast_precision_loss)]
fn aspect_ratio(width: u32, height: u32) -> f32 {
    width.max(1) as f32 / height.max(1) as f32
}

fn wrap_angle(angle: f32) -> f32 {
    (angle + PI).rem_euclid(TAU) - PI
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level_camera() -> Camera {
        let mut camera = Camera::new(1.0);
        camera.yaw = 0.0;
        camera.pitch = 0.0;
        camera
    }

    #[test]
    fn yaw_zero_looks_down_negative_z() {
        let camera = level_camera();

        assert!((camera.forward() - Vec3::NEG_Z).length() < 1e-5);
    }

    #[test]
    fn quarter_turn_looks_positive_x() {
        let mut camera = level_camera();
        camera.yaw = PI / 2.0;

        assert!((camera.forward() - Vec3::X).length() < 1e-5);
    }

    #[test]
    fn pitch_is_clamped() {
        let mut camera = level_camera();

        camera.look(0.0, -1_000_000.0);
        assert!((camera.pitch - MAX_PITCH_RADIANS).abs() < 1e-5);

        camera.look(0.0, 1_000_000.0);
        assert!((camera.pitch + MAX_PITCH_RADIANS).abs() < 1e-5);
    }

    #[test]
    fn yaw_stays_wrapped() {
        let mut camera = level_camera();

        camera.look(1_000_000.0, 0.0);

        assert!((-PI..PI).contains(&camera.yaw));
    }

    #[test]
    fn fractional_position_changes_the_view_projection() {
        let mut camera = level_camera();
        let origin = camera.view_projection();

        camera.set_position(Vec3::new(4.0, 5.0, 6.5));

        assert_ne!(camera.view_projection(), origin);
    }

    #[test]
    fn whole_block_position_keeps_the_relative_view_projection() {
        let mut camera = level_camera();
        let origin = camera.render_view();

        camera.set_position(Vec3::new(4.0, 5.0, 6.0));
        let shifted = camera.render_view();

        assert_eq!(shifted.origin, IVec3::new(4, 5, 6));
        assert_eq!(shifted.view_projection, origin.view_projection);
        assert_eq!(shifted.camera_position, origin.camera_position);
    }

    /// Spec 14.3: at 2^20 blocks the draw inputs stay relative while picking
    /// and collision keep returning world-space results.
    #[test]
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "the fixture uses block-aligned 2^20 coordinates that are exact in f32, and the reference NDC is compared in f32"
    )]
    fn camera_relative_precision_at_two_to_the_twenty() {
        use glam::{DVec3, Vec3 as F32Vec3};

        use crate::block::Block;
        use crate::player::{Player, PlayerInput};
        use crate::raycast::raycast;
        use crate::world::World;

        const BASE: i32 = 1 << 20;
        const BASE_F: f32 = 1_048_576.0;

        let mut camera = level_camera();
        camera.set_position(F32Vec3::new(BASE_F + 0.5, 64.5, BASE_F + 0.5));
        let view = camera.render_view();

        assert_eq!(view.origin, IVec3::new(BASE, 64, BASE));
        assert_eq!(view.camera_position, F32Vec3::splat(0.5));

        // (a) Section and chunk origins near the camera are small even though
        // their world coordinates are ~2^20.
        let section_origin = [BASE + 32, 32, BASE - 32];
        let section_delta = view.relative_block(section_origin);
        assert!(
            section_delta.abs().max_element() < 128.0,
            "section delta {section_delta:?} is not small"
        );
        let chunk = crate::world::ChunkCoord::from_world(IVec3::new(BASE + 16, 40, BASE + 16)).0;
        let chunk_delta = view.relative_point(chunk.origin());
        assert!(
            chunk_delta.abs().max_element() < 64.0,
            "chunk delta {chunk_delta:?} is not small"
        );

        // (b) The relative f32 transform reproduces an f64 world-space
        // reference for a nearby point.
        let nearby_world = F32Vec3::new(BASE_F + 0.25, 64.25, BASE_F - 10.0);
        let clip = view.view_projection * view.relative_point(nearby_world).extend(1.0);
        let reference = glam::dcamera::rh::proj::directx::perspective(
            f64::from(camera.fov_y_radians),
            f64::from(camera.aspect),
            f64::from(camera.z_near),
            f64::from(camera.z_far),
        ) * glam::dcamera::rh::view::look_to_mat4(
            camera.position.as_dvec3(),
            camera.forward().as_dvec3(),
            DVec3::Y,
        );
        let reference_clip = reference * nearby_world.as_dvec3().extend(1.0);
        let relative_ndc = F32Vec3::new(clip.x / clip.w, clip.y / clip.w, clip.z / clip.w);
        let reference_ndc = F32Vec3::new(
            (reference_clip.x / reference_clip.w) as f32,
            (reference_clip.y / reference_clip.w) as f32,
            (reference_clip.z / reference_clip.w) as f32,
        );
        let error = relative_ndc.distance(reference_ndc);
        assert!(
            error < 1e-5,
            "relative NDC {relative_ndc:?} differs from f64 reference {reference_ndc:?} by {error}"
        );

        // (c) Picking and collision keep world-space results. The block is at
        // z = BASE - 8, and the player lands on the block at y = 63.
        let mut world = World::default();
        world.set_block(IVec3::new(BASE, 64, BASE - 8), Block::Stone);
        world.set_block(IVec3::new(BASE, 63, BASE), Block::Stone);
        let hit = raycast(&world, camera.position(), F32Vec3::NEG_Z, 32.0)
            .expect("the fixture block is in front of the camera");
        assert_eq!(hit.block, IVec3::new(BASE, 64, BASE - 8));
        assert_eq!(hit.normal, IVec3::Z);

        let mut player = Player::new(F32Vec3::new(BASE_F + 0.5, 65.0, BASE_F + 0.5));
        let input = PlayerInput {
            forward: 0.0,
            right: 0.0,
            jump: false,
            descend: false,
            yaw: 0.0,
        };
        for _ in 0..120 {
            player.update(&world, input, 1.0 / 60.0);
        }
        assert!(player.on_ground);
        assert!((player.position.y - 64.0).abs() < 1e-3);
        assert!((player.position.x - (BASE_F + 0.5)).abs() < 1e-3);
        assert!((player.position.z - (BASE_F + 0.5)).abs() < 1e-3);
    }
}
