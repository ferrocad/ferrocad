//! The camera handle the gizmo (and picking) need from Python.
//!
//! Everything below the host speaks **device pixels**; a Python script that drives a
//! [`TransformGizmo`](crate::TransformGizmo) needs the same view/projection the renderer
//! uses, so it wraps a [`ferrocad_graphics::Camera`]. The convenience constructors
//! mirror the Rust sample camera (`ferrocad_viewport::scene`): at the origin, looking
//! down −Z, from `eye_z` away.

use ferrocad_graphics as gfx;
use ferrocad_stage as fs;
use ferrocad_types::{Matrix4, Placement, Rotation, Vector3};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

/// A view/projection camera with a device-pixel viewport.
#[pyclass(module = "ferrocad_stage", name = "Camera")]
#[derive(Clone)]
pub struct Camera {
    pub(crate) inner: gfx::Camera,
}

impl Camera {
    /// Wrap a native camera.
    pub(crate) fn wrap(inner: gfx::Camera) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl Camera {
    /// A camera from row-major `view` and `projection` matrices (16 floats each).
    #[new]
    #[pyo3(signature = (view, projection, size=(800, 600), device_pixel_ratio=1.0))]
    fn new(
        view: Vec<f64>,
        projection: Vec<f64>,
        size: (u32, u32),
        device_pixel_ratio: f32,
    ) -> PyResult<Self> {
        Ok(Self {
            inner: gfx::Camera {
                view: matrix(&view, "view")?,
                projection: gfx::Projection::Matrix(matrix(&projection, "projection")?),
                viewport: gfx::Viewport::new([size.0, size.1], device_pixel_ratio),
            },
        })
    }

    /// The identity camera (clip space == world space), useful for tests.
    #[staticmethod]
    fn identity() -> Self {
        Self {
            inner: gfx::Camera::identity(),
        }
    }

    /// An orthographic camera looking down −Z from `(0, 0, eye_z)`, with a `size`-pixel
    /// viewport and a vertical half-extent of `extent`.
    ///
    /// The clip window is deliberately wide (`±10⁴` around the eye): an orthographic
    /// camera may legitimately see geometry behind its eye, and a caller that zooms by
    /// `zoom_projection` keeps the window.
    #[staticmethod]
    #[pyo3(signature = (extent, size=(800, 600), device_pixel_ratio=1.0, eye_z=100.0))]
    fn orthographic(extent: f64, size: (u32, u32), device_pixel_ratio: f32, eye_z: f64) -> Self {
        let view = Placement::new(Vector3::new(0.0, 0.0, -eye_z), Rotation::identity()).to_matrix();
        Self {
            inner: gfx::Camera {
                view,
                projection: gfx::Projection::Orthographic {
                    extent,
                    near: -1.0e4,
                    far: 1.0e4,
                },
                viewport: gfx::Viewport::new([size.0, size.1], device_pixel_ratio),
            },
        }
    }

    /// A 45°-by-default perspective camera at `(0, 0, eye_z)` looking toward the origin,
    /// with a `size`-pixel viewport.
    #[staticmethod]
    #[pyo3(signature = (eye_z, size=(800, 600), device_pixel_ratio=1.0, fov_deg=45.0, near=0.1, far=100.0))]
    fn perspective(
        eye_z: f64,
        size: (u32, u32),
        device_pixel_ratio: f32,
        fov_deg: f64,
        near: f64,
        far: f64,
    ) -> Self {
        // The camera sits at `+eye_z`, looking down −Z, so the world → view matrix is the
        // inverse: a translation by `−eye_z`.
        let view = Placement::new(Vector3::new(0.0, 0.0, -eye_z), Rotation::identity()).to_matrix();
        Self {
            inner: gfx::Camera {
                view,
                projection: gfx::Projection::Perspective {
                    fov_y: fov_deg.to_radians(),
                    near,
                    far,
                },
                viewport: gfx::Viewport::new([size.0, size.1], device_pixel_ratio),
            },
        }
    }

    /// The device-pixel viewport size.
    #[getter]
    fn size(&self) -> (u32, u32) {
        let [w, h] = self.inner.viewport.device_size();
        (w, h)
    }

    /// Whether the projection is orthographic.
    #[getter]
    fn is_orthographic(&self) -> bool {
        self.inner.is_orthographic()
    }

    /// The projection kind: `"perspective"`, `"orthographic"` or `"matrix"`.
    #[getter]
    fn projection_kind(&self) -> &'static str {
        match self.inner.projection {
            gfx::Projection::Perspective { .. } => "perspective",
            gfx::Projection::Orthographic { .. } => "orthographic",
            gfx::Projection::Matrix(_) => "matrix",
        }
    }

    /// The eye position in world space.
    #[getter]
    fn position(&self) -> (f64, f64, f64) {
        triple(fs::screen::camera_position(&self.inner.view))
    }

    /// The world direction the camera looks along (view-space −Z in world space).
    #[getter]
    fn direction(&self) -> (f64, f64, f64) {
        triple(fs::screen::camera_forward(&self.inner))
    }

    /// The camera's screen-right direction in world space.
    #[getter]
    fn right(&self) -> (f64, f64, f64) {
        triple(fs::screen::camera_right(&self.inner))
    }

    /// The camera's screen-up direction in world space.
    #[getter]
    fn up(&self) -> (f64, f64, f64) {
        triple(fs::screen::camera_up(&self.inner))
    }

    /// The camera's orientation as a `(x, y, z, w)` quaternion (Coin `SbRotation`).
    ///
    /// The quaternion maps camera space to world space, so it is the inverse of the
    /// view matrix's rotation — the convention `SoCamera.orientation` uses.
    #[getter]
    fn orientation(&self) -> (f64, f64, f64, f64) {
        let right = fs::screen::camera_right(&self.inner);
        let up = fs::screen::camera_up(&self.inner);
        let back = right.cross(&up);
        quaternion(right, up, back)
    }

    /// The distance from the eye to the pivot (the orbit radius).
    #[getter]
    fn focal_distance(&self) -> f64 {
        self.inner.eye_distance()
    }

    /// The world height of the view at the focal distance.
    ///
    /// A perspective camera's height is `2·d·tan(fov/2)` at distance `d`; an
    /// orthographic camera's is twice its vertical half-extent. An explicit project
    /// matrix has no such parameters, so it reports `0`.
    #[getter]
    fn height(&self) -> f64 {
        match self.inner.projection {
            gfx::Projection::Perspective { fov_y, .. } => {
                2.0 * self.inner.eye_distance() * (fov_y / 2.0).tan()
            }
            gfx::Projection::Orthographic { extent, .. } => 2.0 * extent,
            gfx::Projection::Matrix(_) => 0.0,
        }
    }

    /// The vertical field of view in radians (an orthographic camera reports `0`).
    #[getter]
    fn height_angle(&self) -> f64 {
        match self.inner.projection {
            gfx::Projection::Perspective { fov_y, .. } => fov_y,
            _ => 0.0,
        }
    }

    /// The viewport aspect ratio (`width / height`).
    #[getter]
    fn aspect_ratio(&self) -> f64 {
        self.inner.aspect()
    }

    /// The world → view matrix (16 row-major floats).
    #[getter]
    fn view_matrix(&self) -> Vec<f64> {
        self.inner.view.values().to_vec()
    }

    /// The view → clip matrix (16 row-major floats) for the current viewport aspect.
    #[getter]
    fn projection_matrix(&self) -> Vec<f64> {
        self.inner.projection_matrix().values().to_vec()
    }

    /// Project a world point to device pixels (bottom-left origin).
    ///
    /// `None` when the point is on (or behind) the camera's `w = 0` plane, where a
    /// projection has no screen position.
    fn project(&self, point: (f64, f64, f64)) -> Option<(f64, f64)> {
        fs::screen::project_device(&self.inner, Vector3::new(point.0, point.1, point.2))
            .map(|[x, y]| (x, y))
    }

    /// The near and far world points of the ray through normalized viewport
    /// coordinates `(u, v)`, both in `0..=1` with a **bottom-left** origin.
    ///
    /// This is Coin's `SbViewVolume.projectPointToLine` in FerroCAD's convention. A
    /// singular view matrix has no ray, so it reports `None`.
    fn ray(&self, u: f64, v: f64) -> Option<((f64, f64, f64), (f64, f64, f64))> {
        let inverse = self.inner.view_projection().inverse()?;
        let x = u * 2.0 - 1.0;
        let y = v * 2.0 - 1.0;
        Some((
                triple(unproject(&inverse, x, y, 0.0)),
                triple(unproject(&inverse, x, y, 1.0)),
            ))
    }

    /// The ray through a **device-pixel** position (bottom-left origin), as
    /// `(origin, direction)` with a normalized direction.
    fn ray_at(&self, x: i32, y: i32) -> Option<((f64, f64, f64), (f64, f64, f64))> {
        let ray = self.inner.ray_through([x, y])?;
        Some((triple_f64(ray.origin), triple_f64(ray.direction)))
    }
}

/// Unproject an NDC point with the inverse view-projection (clip depth in `0..=1`).
fn unproject(inverse: &Matrix4, x: f64, y: f64, z: f64) -> Vector3 {
    let m = &inverse.m;
    let out = [
        m[0] * x + m[1] * y + m[2] * z + m[3],
        m[4] * x + m[5] * y + m[6] * z + m[7],
        m[8] * x + m[9] * y + m[10] * z + m[11],
        m[12] * x + m[13] * y + m[14] * z + m[15],
    ];
    let w = if out[3] != 0.0 { out[3] } else { 1.0 };
    Vector3::new(out[0] / w, out[1] / w, out[2] / w)
}

/// The `(x, y, z, w)` quaternion of the rotation whose world-space columns are
/// `right`, `up` and `back` (the trace/Shepperd method).
fn quaternion(right: Vector3, up: Vector3, back: Vector3) -> (f64, f64, f64, f64) {
    // Row-major camera → world rotation: columns are the camera's world axes.
    let r = [
        [right.x, up.x, back.x],
        [right.y, up.y, back.y],
        [right.z, up.z, back.z],
    ];
    let trace = r[0][0] + r[1][1] + r[2][2];
    let (w, x, y, z) = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        (0.25 * s, (r[2][1] - r[1][2]) / s, (r[0][2] - r[2][0]) / s, (r[1][0] - r[0][1]) / s)
    } else if r[0][0] > r[1][1] && r[0][0] > r[2][2] {
        let s = (1.0 + r[0][0] - r[1][1] - r[2][2]).sqrt() * 2.0;
        (0.25 * s, (r[1][0] + r[0][1]) / s, (r[0][2] + r[2][0]) / s, (r[2][1] - r[1][2]) / s)
    } else if r[1][1] > r[2][2] {
        let s = (1.0 + r[1][1] - r[0][0] - r[2][2]).sqrt() * 2.0;
        (0.25 * s, (r[1][0] + r[0][1]) / s, (r[2][1] + r[1][2]) / s, (r[0][2] - r[2][0]) / s)
    } else {
        let s = (1.0 + r[2][2] - r[0][0] - r[1][1]).sqrt() * 2.0;
        (0.25 * s, (r[0][2] + r[2][0]) / s, (r[2][1] + r[1][2]) / s, (r[1][0] - r[0][1]) / s)
    };
    (x, y, z, w)
}

fn triple(v: Vector3) -> (f64, f64, f64) {
    (v.x, v.y, v.z)
}

fn triple_f64(v: [f64; 3]) -> (f64, f64, f64) {
    (v[0], v[1], v[2])
}

/// A row-major 4×4 matrix from 16 floats.
fn matrix(values: &[f64], what: &str) -> PyResult<Matrix4> {
    let values: [f64; 16] = values.try_into().map_err(|_| {
        PyValueError::new_err(format!("{what} must be 16 floats, got {}", values.len()))
    })?;
    Ok(Matrix4::from_values(values))
}

/// Register this module's classes.
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Camera>()?;
    Ok(())
}
