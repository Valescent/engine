//! First-person camera state and look direction.

use std::f32::consts::{PI, TAU};

use glam::camera::rh::{proj::directx, view::look_to_mat4};
use glam::{Mat4, Vec3};

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
            z_far: 100.0,
            sensitivity: 0.0025,
        }
    }

    pub(crate) fn set_aspect(&mut self, width: u32, height: u32) {
        self.aspect = aspect_ratio(width, height);
    }

    pub(crate) fn set_position(&mut self, position: Vec3) {
        self.position = position;
    }

    pub(crate) fn yaw(&self) -> f32 {
        self.yaw
    }

    pub(crate) fn look(&mut self, delta_x: f32, delta_y: f32) {
        self.yaw = wrap_angle(self.yaw + delta_x * self.sensitivity);
        self.pitch =
            (self.pitch - delta_y * self.sensitivity).clamp(-MAX_PITCH_RADIANS, MAX_PITCH_RADIANS);
    }

    pub(crate) fn view_projection(&self) -> Mat4 {
        directx::perspective(self.fov_y_radians, self.aspect, self.z_near, self.z_far)
            * look_to_mat4(self.position, self.forward(), Vec3::Y)
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
    fn position_changes_the_view_projection() {
        let mut camera = level_camera();
        let origin = camera.view_projection();

        camera.set_position(Vec3::new(4.0, 5.0, 6.0));

        assert_ne!(camera.view_projection(), origin);
    }
}
