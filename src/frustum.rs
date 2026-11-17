//! Six-plane view frustum extracted from the camera's view-projection matrix.
//!
//! wgpu uses a `0..1` depth range, so the near plane is the second matrix row
//! rather than the usual Gribb-Hartmann difference. Plane normals point into the
//! frustum, and the AABB test is conservative: nothing visible is rejected.

use glam::{Mat4, Vec3};

/// One normalized frustum plane, `dot(normal, point) + distance = 0` on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Plane {
    pub(crate) normal: Vec3,
    pub(crate) distance: f32,
}

/// The six planes of a view frustum.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Frustum {
    pub(crate) planes: [Plane; 6],
}

impl Frustum {
    /// Extracts and normalizes the six planes of a view-projection matrix.
    pub(crate) fn from_view_projection(view_projection: Mat4) -> Self {
        let row = |index: usize| view_projection.row(index);
        let (left, right) = (row(0), row(3));

        Self {
            planes: [
                Plane::new(left + right),
                Plane::new(row(3) - row(0)),
                Plane::new(row(1) + row(3)),
                Plane::new(row(3) - row(1)),
                Plane::new(row(2)),
                Plane::new(row(3) - row(2)),
            ],
        }
    }

    /// Returns whether an axis-aligned box may be visible.
    ///
    /// The box is rejected only when it lies entirely behind a plane.
    pub(crate) fn intersects_aabb(&self, min: Vec3, max: Vec3) -> bool {
        self.planes.iter().all(|plane| {
            let farthest = Vec3::new(
                if plane.normal.x >= 0.0 { max.x } else { min.x },
                if plane.normal.y >= 0.0 { max.y } else { min.y },
                if plane.normal.z >= 0.0 { max.z } else { min.z },
            );
            plane.normal.dot(farthest) + plane.distance >= 0.0
        })
    }
}

impl Plane {
    fn new(coefficients: glam::Vec4) -> Self {
        let normal = coefficients.truncate();
        let length = normal.length();
        if length == 0.0 {
            return Self {
                normal: Vec3::ZERO,
                distance: 0.0,
            };
        }
        Self {
            normal: normal / length,
            distance: coefficients.w / length,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_4;

    use glam::camera::rh::{proj::directx, view::look_to_mat4};

    use super::*;

    fn camera_frustum() -> Frustum {
        let projection = directx::perspective(FRAC_PI_4, 1.0, 0.1, 100.0);
        let view = look_to_mat4(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y);
        Frustum::from_view_projection(projection * view)
    }

    #[test]
    fn planes_are_normalized() {
        for plane in camera_frustum().planes {
            assert!((plane.normal.length() - 1.0).abs() < 1e-5, "{plane:?}");
        }
    }

    #[test]
    fn a_box_in_front_is_visible() {
        let frustum = camera_frustum();

        assert!(frustum.intersects_aabb(Vec3::new(-1.0, -1.0, -20.0), Vec3::new(1.0, 1.0, -18.0)));
    }

    #[test]
    fn a_box_behind_the_camera_is_rejected() {
        let frustum = camera_frustum();

        assert!(!frustum.intersects_aabb(Vec3::new(-1.0, -1.0, 18.0), Vec3::new(1.0, 1.0, 20.0)));
    }

    #[test]
    fn a_box_far_to_the_side_is_rejected() {
        let frustum = camera_frustum();

        assert!(!frustum.intersects_aabb(
            Vec3::new(-200.0, -1.0, -20.0),
            Vec3::new(-180.0, 1.0, -18.0)
        ));
    }

    #[test]
    fn a_box_past_the_far_plane_is_rejected() {
        let frustum = camera_frustum();

        assert!(
            !frustum.intersects_aabb(Vec3::new(-1.0, -1.0, -300.0), Vec3::new(1.0, 1.0, -280.0))
        );
    }

    #[test]
    fn a_straddling_box_is_kept() {
        let frustum = camera_frustum();

        assert!(frustum.intersects_aabb(Vec3::new(-2.0, -2.0, -2.0), Vec3::splat(2.0)));
    }

    #[test]
    fn a_box_containing_the_camera_is_kept() {
        let frustum = camera_frustum();

        assert!(frustum.intersects_aabb(Vec3::splat(-100.0), Vec3::splat(100.0)));
    }
}
