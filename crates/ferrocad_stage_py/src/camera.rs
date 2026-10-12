//! The camera handle the gizmo (and picking) need from Python.
//!
//! Everything below the host speaks **device pixels**; a Python script that drives a
//! [`TransformGizmo`](crate::TransformGizmo) needs the same view/projection the renderer
//! uses, so it wraps a [`ferrocad_graphics::Camera`]. The convenience constructors
//! mirror the Rust sample camera (`ferrocad_viewport::scene`): at the origin, looking
//! down −Z, from `eye_z` away.

use ferrocad_graphics as gfx;
use ferrocad_types::{Matrix4, Placement, Rotation, Vector3};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

/// A view/projection camera with a device-pixel viewport.
#[pyclass(module = "ferrocad_stage", name = "Camera")]
#[derive(Clone)]
pub struct Camera {
    pub(crate) inner: gfx::Camera,
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
