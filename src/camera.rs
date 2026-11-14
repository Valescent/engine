//! First-person camera state and movement.

use glam::camera::rh::{proj::directx, view::look_to_mat4};
use glam::{Mat4, Vec3};

use std::f32::consts::{PI, TAU};

const MAX_PITCH_RADIANS: f32 = 89.0 * PI / 180.0;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Camera {
    position: Vec3,
    yaw: f32,
    pitch: f32,
    aspect: f32,
    fov_y_radians: f32,
    z_near: f32,
    z_far: f32,
    speed: f32,
    sensitivity: f32,
}

impl Camera {
    pub(crate) fn new(aspect: f32) -> Self {
        let position = Vec3::new(6.0, 4.5, 7.0);
        let target = Vec3::new(1.0, 0.5, 1.0);
        let direction = (target - position).normalize();

        Self {
            position,
            yaw: direction.x.atan2(-direction.z),
            pitch: direction.y.asin(),
            aspect,
            fov_y_radians: std::f32::consts::FRAC_PI_3,
            z_near: 0.1,
            z_far: 100.0,
            speed: 8.0,
            sensitivity: 0.0025,
        }
    }

    pub(crate) fn set_aspect(&mut self, width: u32, height: u32) {
        self.aspect = aspect_ratio(width, height);
    }

    pub(crate) fn look(&mut self, delta_x: f32, delta_y: f32) {
        self.yaw = wrap_angle(self.yaw + delta_x * self.sensitivity);
        self.pitch =
            (self.pitch - delta_y * self.sensitivity).clamp(-MAX_PITCH_RADIANS, MAX_PITCH_RADIANS);
    }

    pub(crate) fn move_by(&mut self, forward_axis: f32, right_axis: f32, delta_seconds: f32) {
        let direction =
            (self.forward() * forward_axis + self.right() * right_axis).normalize_or_zero();
        self.position += direction * self.speed * delta_seconds;
    }

    pub(crate) fn view_projection(&self) -> Mat4 {
        directx::perspective(self.fov_y_radians, self.aspect, self.z_near, self.z_far)
            * look_to_mat4(self.position, self.forward(), Vec3::Y)
    }

    fn forward(&self) -> Vec3 {
        let (yaw_sin, yaw_cos) = self.yaw.sin_cos();
        let (pitch_sin, pitch_cos) = self.pitch.sin_cos();
        Vec3::new(yaw_sin * pitch_cos, pitch_sin, -yaw_cos * pitch_cos)
    }

    fn right(&self) -> Vec3 {
        let (yaw_sin, yaw_cos) = self.yaw.sin_cos();
        Vec3::new(yaw_cos, 0.0, yaw_sin)
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
    fn initial_camera_looks_at_the_scene() {
        let camera = Camera::new(1.0);
        let target_direction = (Vec3::new(1.0, 0.5, 1.0) - camera.position).normalize();

        assert!(camera.forward().dot(target_direction) > 0.999);
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
    fn movement_is_frame_rate_independent() {
        let mut one_step = level_camera();
        let mut two_steps = level_camera();

        one_step.move_by(1.0, 0.0, 1.0);
        two_steps.move_by(1.0, 0.0, 0.5);
        two_steps.move_by(1.0, 0.0, 0.5);

        assert!((one_step.position - two_steps.position).length() < 1e-5);
        assert!((one_step.position - Vec3::new(6.0, 4.5, -1.0)).length() < 1e-5);
    }
}
