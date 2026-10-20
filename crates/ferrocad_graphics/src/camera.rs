//! The camera: the view/projection a renderer needs, renderer-agnostic.
//!
//! Matrices use [`ferrocad_types::Matrix4`] so the camera is usable by any backend;
//! a backend converts to its own uniform layout (e.g. `f32` columns for wgpu). The
//! [`Viewport`] carries the physical size and scale factor, from which the host
//! derives event and target coordinates — and the *aspect* the [`Projection`] is
//! rebuilt from, so the projection follows a resize instead of being baked in.
//!
//! Clip depth is in `[0, 1]` (the wgpu/WebGPU convention), matching the renderer.

use ferrocad_types::{Matrix4, Vector3};

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

    /// The distance from the eye to the pivot (the world origin).
    ///
    /// The camera orbits the origin, so the world point the view maps to the view-space
    /// origin is the eye; its length is the orbit radius. A singular view (no inverse)
    /// reports zero.
    pub fn eye_distance(&self) -> f64 {
        self.view
            .inverse()
            .map(|inverse| inverse.transform(&Vector3::zero()).length())
            .unwrap_or(0.0)
    }

    /// Whether the camera is currently orthographic.
    pub fn is_orthographic(&self) -> bool {
        matches!(self.projection, Projection::Orthographic { .. })
    }

    /// Replace the projection, reporting whether it changed.
    pub fn set_projection(&mut self, projection: Projection) -> bool {
        if self.projection == projection {
            return false;
        }
        self.projection = projection;
        true
    }

    /// Toggle perspective ⇄ orthographic, **preserving the apparent size at the pivot**.
    ///
    /// The camera orbits the world origin at [`eye_distance`](Self::eye_distance), where
    /// a perspective camera shows a half-height of `distance · tan(fov_y / 2)`. The
    /// orthographic `extent` is set to that, and the inverse restores the same field of
    /// view, so toggling does not jump the framing. A host-supplied
    /// [`Projection::Matrix`] is left alone; the return value says whether it changed.
    pub fn toggle_projection(&mut self) -> bool {
        let distance = self.eye_distance();
        if distance <= 1e-9 {
            return false;
        }
        self.projection = match self.projection {
            Projection::Perspective { fov_y, near, far } => Projection::Orthographic {
                extent: distance * (fov_y / 2.0).tan(),
                near,
                far,
            },
            Projection::Orthographic { extent, near, far } => Projection::Perspective {
                fov_y: 2.0 * (extent / distance).atan(),
                near,
                far,
            },
            Projection::Matrix(_) => return false,
        };
        true
    }

    /// Scale an orthographic view by `factor` — the zoom equivalent of moving the eye in
    /// perspective. Perspective and explicit matrices are left alone (their zoom is the
    /// eye distance). Returns whether the projection changed.
    pub fn zoom_projection(&mut self, factor: f64) -> bool {
        if !factor.is_finite() || factor <= 0.0 {
            return false;
        }
        if let Projection::Orthographic { extent, .. } = &mut self.projection {
            *extent = (*extent * factor).clamp(1e-6, 1e9);
            return true;
        }
        false
    }

    /// Recompute the clip planes from the camera's current distance (and, for an
    /// orthographic camera, its extent), so zooming in or out cannot clip the scene.
    ///
    /// The zoom paths move the eye (perspective) or scale the extent (orthographic) but
    /// keep the projection's `near`/`far`; a fixed `0.1 .. 100` window then cuts the near
    /// face off when you zoom in and the far side away when you zoom out. The rule here is
    /// relative to `d = eye_distance`:
    ///
    /// - perspective: `near = d / 1000`, `far = d * 1000`, with a `1e-4` floor on `near`;
    /// - orthographic: the eye is held at `d`, so the window spans `d ± 4·extent` (a
    ///   parallel camera may legitimately see geometry behind its eye, so `near` can be
    ///   negative).
    ///
    /// Returns whether either plane moved. A host-supplied [`Projection::Matrix`] is left
    /// alone (its clip planes are the host's business).
    pub fn fit_clip(&mut self) -> bool {
        let distance = self.eye_distance().max(1e-6);
        let (near, far) = match self.projection {
            Projection::Perspective { .. } => {
                let near = (distance / 1_000.0).max(1e-4);
                (near, (distance * 1_000.0).max(near * 10.0))
            }
            Projection::Orthographic { extent, .. } => {
                let reach = extent.abs().max(1e-3) * 4.0;
                (distance - reach, distance + reach)
            }
            Projection::Matrix(_) => return false,
        };
        match &mut self.projection {
            Projection::Perspective {
                near: n, far: f, ..
            }
            | Projection::Orthographic {
                near: n, far: f, ..
            } => {
                if (*n - near).abs() < 1e-12 && (*f - far).abs() < 1e-12 {
                    return false;
                }
                *n = near;
                *f = far;
                true
            }
            Projection::Matrix(_) => false,
        }
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
    fn fitting_the_clip_follows_the_zoom() {
        use ferrocad_types::{Placement, Rotation, Vector3};

        let at = |distance: f64| Camera {
            view: Placement::new(Vector3::new(0.0, 0.0, -distance), Rotation::identity())
                .to_matrix(),
            projection: Projection::Perspective {
                fov_y: 45.0_f64.to_radians(),
                near: 0.1,
                far: 100.0,
            },
            viewport: Viewport::new([100, 100], 1.0),
        };

        // At the default distance the window is generous in both directions.
        let mut camera = at(4.0);
        assert!(camera.fit_clip());
        let Projection::Perspective { near, far, .. } = camera.projection else {
            panic!("perspective")
        };
        assert!(near < 0.01 && far > 400.0, "{near}..{far}");
        assert!(!camera.fit_clip(), "a second call is a no-op");

        // Zoomed right in, the near plane follows instead of clipping at 0.1.
        let mut camera = at(0.01);
        camera.fit_clip();
        let Projection::Perspective { near, far, .. } = camera.projection else {
            panic!("perspective")
        };
        assert!(near <= 1e-4 + 1e-12 && near < 0.01, "near {near}");
        assert!(far > 0.01, "far {far}");

        // Zoomed far out, the far plane follows instead of clipping at 100.
        let mut camera = at(5_000.0);
        camera.fit_clip();
        let Projection::Perspective { near, far, .. } = camera.projection else {
            panic!("perspective")
        };
        assert!(near < 5_000.0 && far > 5_000.0, "{near}..{far}");

        // Orthographic: the eye is held, so the window spans around it (behind too).
        let mut camera = at(4.0);
        camera.projection = Projection::Orthographic {
            extent: 2.0,
            near: 0.1,
            far: 100.0,
        };
        camera.fit_clip();
        let Projection::Orthographic { near, far, .. } = camera.projection else {
            panic!("orthographic")
        };
        assert!(near < 0.0 && near < 4.0 && far > 4.0, "{near}..{far}");
    }

    #[test]
    fn toggling_projection_preserves_the_framing_at_the_pivot() {
        use ferrocad_types::Placement;

        let fov_y = 45.0_f64.to_radians();
        let mut camera = Camera {
            // The eye sits four units from the pivot (the world origin).
            view: Placement::new(Vector3::new(0.0, 0.0, -4.0), ferrocad_types::Rotation::identity())
                .to_matrix(),
            projection: Projection::Perspective {
                fov_y,
                near: 0.1,
                far: 100.0,
            },
            viewport: Viewport::new([100, 100], 1.0),
        };
        assert!((camera.eye_distance() - 4.0).abs() < 1e-9);

        assert!(camera.toggle_projection());
        assert!(camera.is_orthographic());
        match camera.projection {
            Projection::Orthographic { extent, .. } => {
                let expected = 4.0 * (fov_y / 2.0).tan();
                assert!((extent - expected).abs() < 1e-9, "extent matches the pivot plane");
            }
            _ => panic!("expected orthographic"),
        }

        // ...and back again, to the same field of view.
        assert!(camera.toggle_projection());
        match camera.projection {
            Projection::Perspective { fov_y: restored, .. } => {
                assert!((restored - fov_y).abs() < 1e-9, "round-trips the fov");
            }
            _ => panic!("expected perspective"),
        }
    }

    #[test]
    fn a_matrix_projection_is_not_toggled() {
        let mut camera = Camera::identity();
        let before = camera.projection;
        assert!(!camera.toggle_projection());
        assert_eq!(camera.projection, before);
        assert!(!camera.set_projection(before));
    }

    #[test]
    fn orthographic_zoom_scales_the_extent_only() {
        let mut camera = camera(
            Projection::Orthographic {
                extent: 2.0,
                near: 0.1,
                far: 100.0,
            },
            [100, 100],
        );
        assert!(camera.zoom_projection(0.5));
        assert_eq!(
            camera.projection,
            Projection::Orthographic {
                extent: 1.0,
                near: 0.1,
                far: 100.0,
            }
        );
        assert!(!camera.zoom_projection(0.0));
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
