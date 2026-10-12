//! The camera: the view/projection a renderer needs, renderer-agnostic.
//!
//! Matrices use [`ferrocad_types::Matrix4`] so the camera is usable by any backend;
//! a backend converts to its own uniform layout (e.g. `f32` columns for wgpu). The
//! [`Viewport`] carries the physical size and scale factor, from which the host
//! derives event and target coordinates.

use ferrocad_types::Matrix4;

use crate::pixels::Viewport;

/// A view/projection pair with the render viewport.
#[derive(Clone, Debug)]
pub struct Camera {
    /// The world → view matrix.
    pub view: Matrix4,
    /// The view → clip (projection) matrix.
    pub projection: Matrix4,
    /// The render viewport: physical size and display scale factor.
    pub viewport: Viewport,
}

impl Camera {
    /// A camera with identity view/projection and a 1×1 viewport at a scale factor of 1.
    pub fn identity() -> Self {
        Self {
            view: Matrix4::identity(),
            projection: Matrix4::identity(),
            viewport: Viewport::default(),
        }
    }

    /// The combined world → clip matrix (`projection * view`).
    pub fn view_projection(&self) -> Matrix4 {
        self.projection.mul(&self.view)
    }
}
