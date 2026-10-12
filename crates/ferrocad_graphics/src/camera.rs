//! The camera: the view/projection a renderer needs, renderer-agnostic.
//!
//! Matrices use [`ferrocad_types::Matrix4`] so the camera is usable by any backend;
//! a backend converts to its own uniform layout (e.g. `f32` columns for wgpu). The
//! [`Viewport`] carries the physical size and scale factor, from which the host
//! derives event and target coordinates — and the *aspect* the [`Projection`] is
//! rebuilt from, so the projection follows a resize instead of being baked in.
//!
//! Clip depth is in `[0, 1]` (the wgpu/WebGPU convention), matching the renderer.

use ferrocad_types::Matrix4;

use crate::pixels::Viewport;

/// How the camera maps view space to clip space.
///
/// Stored as **parameters**, not a matrix, so the projection is rebuilt for the current
/// [`Viewport`] aspect on demand (`Camera::projection_matrix`). [`Matrix`](Self::Matrix)
/// is the escape hatch for a projection the host builds itself (its aspect is used as
/// given, not adjusted).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Projection {
    /// A symmetric right-handed perspective projection about the view's −Z axis.
    Perspective {
        /// The vertical field of view, in radians.
        fov_y: f64,
        /// The near plane distance.
        near: f64,
        /// The far plane distance.
        far: f64,
    },
    /// A symmetric orthographic projection: the clip volume spans `[-extent, extent]` on
    /// the (shorter) Y axis, and the aspect widens X.
    Orthographic {
        /// The vertical half-extent, in view units.
        extent: f64,
        /// The near plane distance.
        near: f64,
        /// The far plane distance.
        far: f64,
    },
    /// An explicit projection matrix, used verbatim.
    Matrix(Matrix4),
}

impl Projection {
    /// The projection matrix for a viewport of `aspect` (`width / height`), with clip
    /// depth in `[0, 1]`.
    pub fn matrix(&self, aspect: f64) -> Matrix4 {
        match *self {
            Projection::Perspective { fov_y, near, far } => perspective(fov_y, near, far, aspect),
            Projection::Orthographic { extent, near, far } => orthographic(extent, near, far, aspect),
            Projection::Matrix(matrix) => matrix,
        }
    }
}

/// A view/projection pair with the render viewport.
#[derive(Clone, Debug)]
pub struct Camera {
    /// The world → view matrix.
    pub view: Matrix4,
    /// The projection parameters (rebuilt for the viewport aspect on demand).
    pub projection: Projection,
    /// The render viewport: physical size and display scale factor.
    pub viewport: Viewport,
}

impl Camera {
    /// A camera with identity view/projection and a 1×1 viewport at a scale factor of 1.
    pub fn identity() -> Self {
        Self {
            view: Matrix4::identity(),
            projection: Projection::Matrix(Matrix4::identity()),
            viewport: Viewport::default(),
        }
    }

    /// The viewport aspect ratio (`width / height`).
    pub fn aspect(&self) -> f64 {
        let [width, height] = self.viewport.device_size();
        f64::from(width.max(1)) / f64::from(height.max(1))
    }

    /// The view → clip matrix for the current viewport aspect.
    pub fn projection_matrix(&self) -> Matrix4 {
        self.projection.matrix(self.aspect())
    }

    /// The combined world → clip matrix (`projection * view`).
    pub fn view_projection(&self) -> Matrix4 {
        self.projection_matrix().mul(&self.view)
    }
}

/// A symmetric perspective projection with clip depth in `[0, 1]`.
fn perspective(fov_y: f64, near: f64, far: f64, aspect: f64) -> Matrix4 {
    let f = 1.0 / (fov_y / 2.0).tan();
    let mut m = [0.0; 16];
    m[0] = f / aspect;
    m[5] = f;
    m[10] = far / (near - far);
    m[11] = (far * near) / (near - far);
    m[14] = -1.0;
    Matrix4::from_values(m)
}

/// A symmetric orthographic projection with clip depth in `[0, 1]`.
fn orthographic(extent: f64, near: f64, far: f64, aspect: f64) -> Matrix4 {
    let mut m = [0.0; 16];
    m[0] = 1.0 / (extent * aspect);
    m[5] = 1.0 / extent;
    m[10] = 1.0 / (near - far);
    m[11] = near / (near - far);
    m[15] = 1.0;
    Matrix4::from_values(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera(projection: Projection, size: [u32; 2]) -> Camera {
        Camera {
            view: Matrix4::identity(),
            projection,
            viewport: Viewport::new(size, 1.0),
        }
    }

    #[test]
    fn identity_camera_is_an_identity_projection() {
        let view_projection = Camera::identity().view_projection().values();
        assert_eq!(view_projection, Matrix4::identity().values());
    }

    #[test]
    fn the_perspective_projection_follows_the_aspect() {
        let projection = Projection::Perspective {
            fov_y: 45.0f64.to_radians(),
            near: 0.1,
            far: 100.0,
        };
        // m[0] = f / aspect, m[5] = f: doubling the aspect halves the horizontal scale,
        // leaving the vertical scale alone.
        let square = camera(projection, [100, 100]).projection_matrix();
        let wide = camera(projection, [200, 100]).projection_matrix();
        assert!((square.m[0] - square.m[5]).abs() < 1e-12, "square: m0 == m5");
        assert!((wide.m[0] - square.m[5] / 2.0).abs() < 1e-12, "wide: m0 = m5/2");
        assert!((wide.m[5] - square.m[5]).abs() < 1e-12, "the vertical scale is unchanged");
    }

    #[test]
    fn a_perspective_maps_near_to_zero_and_far_to_one() {
        let projection = Projection::Perspective {
            fov_y: 45.0f64.to_radians(),
            near: 0.1,
            far: 100.0,
        }
        .matrix(1.0);
        let depth = |view_z: f64| {
            let clip_z = projection.m[10] * view_z + projection.m[11];
            let w = -view_z;
            clip_z / w
        };
        assert!((depth(-0.1) - 0.0).abs() < 1e-9, "near -> 0");
        assert!((depth(-100.0) - 1.0).abs() < 1e-9, "far -> 1");
    }

    #[test]
    fn the_orthographic_extent_widens_with_the_aspect() {
        let projection = Projection::Orthographic {
            extent: 2.0,
            near: 0.1,
            far: 100.0,
        };
        let square = camera(projection, [100, 100]).projection_matrix();
        let wide = camera(projection, [400, 100]).projection_matrix();
        assert!((square.m[0] - 0.5).abs() < 1e-12, "square: 1/extent");
        assert!((square.m[5] - 0.5).abs() < 1e-12);
        assert!((wide.m[0] - 0.5 / 4.0).abs() < 1e-12, "wide: x scaled by the aspect");
        assert!((wide.m[5] - 0.5).abs() < 1e-12, "y unchanged");
    }
}
