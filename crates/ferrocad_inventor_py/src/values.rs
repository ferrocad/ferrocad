//! Coin's value classes (`SbVec2f`, `SbVec3f`, `SbRotation`) and the camera nodes.
//!
//! A workbench that drives the viewport by hand builds its own `SbVec2f` for a pick
//! ray (`gui/snapping.py:29`) and asks the camera for its type with
//! `coin.SoPerspectiveCamera.getClassTypeId()` (`gui/snapping.py:31`,
//! `gui/box_selection.py:95`). Both are Coin surface, not FerroCAD surface, so they
//! belong in the facade rather than in the Python adapter.
//!
//! The camera classes are real [`SoNode`]s with a [`NodeKind::Custom`] kind: they can
//! be added to a scene graph like any other node (it draws nothing, which is what the
//! host's own camera is for) and they carry the fields a workbench reads —
//! `position`, `orientation`, `focalDistance`, `height`, `heightAngle`, `aspectRatio`
//! and the clip distances.
//!
//! Deliberately absent (the façade names what it cannot do instead of faking it):
//! field **connections** (`connectFrom`) and the sensor classes (`SoFieldSensor`,
//! `SoNodeSensor`). A camera whose fields were copied once would look connected while
//! staying frozen, which is worse than an `AttributeError`.

use std::sync::{Arc, Mutex};

use ferrocad_inventor::NodeKind;
use pyo3::exceptions::{PyIndexError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyTuple, PyType};
use pyo3::IntoPyObjectExt;

use crate::{SoNode, SoType, derives_from, node_initializer};

// ---------------------------------------------------------------------------
// SbVec2f / SbVec3f
// ---------------------------------------------------------------------------

/// A 2-float vector (Coin's `SbVec2f`).
#[pyclass]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SbVec2f {
    x: f32,
    y: f32,
}

#[pymethods]
impl SbVec2f {
    #[new]
    #[pyo3(signature = (x=0.0, y=0.0))]
    fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    #[getter]
    fn x(&self) -> f32 {
        self.x
    }

    #[setter]
    fn set_x(&mut self, value: f32) {
        self.x = value;
    }

    #[getter]
    fn y(&self) -> f32 {
        self.y
    }

    #[setter]
    fn set_y(&mut self, value: f32) {
        self.y = value;
    }

    /// `vec.getValue()` — the two components as a tuple.
    #[pyo3(name = "getValue")]
    fn get_value(&self) -> (f32, f32) {
        (self.x, self.y)
    }

    /// `vec.setValue(x, y)`, or `vec.setValue((x, y))`.
    #[pyo3(name = "setValue")]
    #[pyo3(signature = (x, y=None))]
    fn set_value(&mut self, x: &Bound<'_, PyAny>, y: Option<f32>) -> PyResult<()> {
        let (x, y) = pair(x, y, "SbVec2f")?;
        self.x = x;
        self.y = y;
        Ok(())
    }

    /// The dot product with another vector.
    #[pyo3(name = "dot")]
    fn dot(&self, other: &SbVec2f) -> f32 {
        self.x * other.x + self.y * other.y
    }

    /// The Euclidean length.
    #[pyo3(name = "length")]
    fn length(&self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }

    /// Normalise in place; a zero vector is left alone (as Coin does).
    #[pyo3(name = "normalize")]
    fn normalize(mut slf: PyRefMut<'_, Self>) -> PyRefMut<'_, Self> {
        let length = (slf.x * slf.x + slf.y * slf.y).sqrt();
        if length > 0.0 {
            slf.x /= length;
            slf.y /= length;
        }
        slf
    }

    fn __len__(&self) -> usize {
        2
    }

    fn __getitem__(&self, index: isize) -> PyResult<f32> {
        match index {
            0 | -2 => Ok(self.x),
            1 | -1 => Ok(self.y),
            _ => Err(PyIndexError::new_err("SbVec2f index out of range")),
        }
    }

    fn __setitem__(&mut self, index: isize, value: f32) -> PyResult<()> {
        match index {
            0 | -2 => self.x = value,
            1 | -1 => self.y = value,
            _ => return Err(PyIndexError::new_err("SbVec2f index out of range")),
        }
        Ok(())
    }

    fn __iter__(slf: PyRef<'_, Self>) -> PyResult<Py<Vec2Iter>> {
        let values = vec![slf.x, slf.y];
        Py::new(slf.py(), Vec2Iter {
            values: values.into_iter(),
        })
    }

    fn __add__(&self, other: &SbVec2f) -> SbVec2f {
        SbVec2f {
            x: self.x + other.x,
            y: self.y + other.y,
        }
    }

    fn __sub__(&self, other: &SbVec2f) -> SbVec2f {
        SbVec2f {
            x: self.x - other.x,
            y: self.y - other.y,
        }
    }

    fn __mul__(&self, factor: f32) -> SbVec2f {
        SbVec2f {
            x: self.x * factor,
            y: self.y * factor,
        }
    }

    fn __neg__(&self) -> SbVec2f {
        SbVec2f {
            x: -self.x,
            y: -self.y,
        }
    }

    fn __repr__(&self) -> String {
        format!("SbVec2f({}, {})", self.x, self.y)
    }
}

/// An iterator over an `SbVec2f`'s components.
#[pyclass]
struct Vec2Iter {
    values: std::vec::IntoIter<f32>,
}

#[pymethods]
impl Vec2Iter {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(mut slf: PyRefMut<'_, Self>) -> Option<f32> {
        slf.values.next()
    }
}

/// A 3-float vector (Coin's `SbVec3f`).
#[pyclass]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SbVec3f {
    x: f32,
    y: f32,
    z: f32,
}

impl SbVec3f {
    fn to_vector(self) -> ferrocad_types::Vector3 {
        ferrocad_types::Vector3::new(f64::from(self.x), f64::from(self.y), f64::from(self.z))
    }

    fn from_vector(vector: ferrocad_types::Vector3) -> Self {
        Self {
            x: vector.x as f32,
            y: vector.y as f32,
            z: vector.z as f32,
        }
    }
}

#[pymethods]
impl SbVec3f {
    #[new]
    #[pyo3(signature = (x=0.0, y=0.0, z=0.0))]
    fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    #[getter]
    fn x(&self) -> f32 {
        self.x
    }

    #[setter]
    fn set_x(&mut self, value: f32) {
        self.x = value;
    }

    #[getter]
    fn y(&self) -> f32 {
        self.y
    }

    #[setter]
    fn set_y(&mut self, value: f32) {
        self.y = value;
    }

    #[getter]
    fn z(&self) -> f32 {
        self.z
    }

    #[setter]
    fn set_z(&mut self, value: f32) {
        self.z = value;
    }

    /// `vec.getValue()` — the three components as a tuple.
    #[pyo3(name = "getValue")]
    fn get_value(&self) -> (f32, f32, f32) {
        (self.x, self.y, self.z)
    }

    /// `vec.setValue(x, y, z)`, or `vec.setValue((x, y, z))`.
    #[pyo3(name = "setValue")]
    #[pyo3(signature = (x, y=None, z=None))]
    fn set_value(
        &mut self,
        x: &Bound<'_, PyAny>,
        y: Option<f32>,
        z: Option<f32>,
    ) -> PyResult<()> {
        let triple = triple(x, y, z, "SbVec3f")?;
        self.x = triple.0;
        self.y = triple.1;
        self.z = triple.2;
        Ok(())
    }

    /// The dot product with another vector.
    #[pyo3(name = "dot")]
    fn dot(&self, other: &SbVec3f) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    /// The cross product with another vector.
    #[pyo3(name = "cross")]
    fn cross(&self, other: &SbVec3f) -> SbVec3f {
        SbVec3f::from_vector(self.to_vector().cross(&other.to_vector()))
    }

    /// The Euclidean length.
    #[pyo3(name = "length")]
    fn length(&self) -> f32 {
        self.to_vector().length() as f32
    }

    /// Normalise in place; a zero vector is left alone (as Coin does).
    #[pyo3(name = "normalize")]
    fn normalize(mut slf: PyRefMut<'_, Self>) -> PyRefMut<'_, Self> {
        let vector = slf.to_vector();
        if !vector.is_null() {
            let unit = vector.normalize();
            slf.x = unit.x as f32;
            slf.y = unit.y as f32;
            slf.z = unit.z as f32;
        }
        slf
    }

    fn __len__(&self) -> usize {
        3
    }

    fn __getitem__(&self, index: isize) -> PyResult<f32> {
        match index {
            0 | -3 => Ok(self.x),
            1 | -2 => Ok(self.y),
            2 | -1 => Ok(self.z),
            _ => Err(PyIndexError::new_err("SbVec3f index out of range")),
        }
    }

    fn __setitem__(&mut self, index: isize, value: f32) -> PyResult<()> {
        match index {
            0 | -3 => self.x = value,
            1 | -2 => self.y = value,
            2 | -1 => self.z = value,
            _ => return Err(PyIndexError::new_err("SbVec3f index out of range")),
        }
        Ok(())
    }

    fn __iter__(slf: PyRef<'_, Self>) -> PyResult<Py<Vec3Iter>> {
        let values = vec![slf.x, slf.y, slf.z];
        Py::new(slf.py(), Vec3Iter {
            values: values.into_iter(),
        })
    }

    fn __add__(&self, other: &SbVec3f) -> SbVec3f {
        SbVec3f {
            x: self.x + other.x,
            y: self.y + other.y,
            z: self.z + other.z,
        }
    }

    fn __sub__(&self, other: &SbVec3f) -> SbVec3f {
        SbVec3f {
            x: self.x - other.x,
            y: self.y - other.y,
            z: self.z - other.z,
        }
    }

    fn __mul__(&self, factor: f32) -> SbVec3f {
        SbVec3f {
            x: self.x * factor,
            y: self.y * factor,
            z: self.z * factor,
        }
    }

    fn __neg__(&self) -> SbVec3f {
        SbVec3f {
            x: -self.x,
            y: -self.y,
            z: -self.z,
        }
    }

    fn __repr__(&self) -> String {
        format!("SbVec3f({}, {}, {})", self.x, self.y, self.z)
    }
}

/// An iterator over an `SbVec3f`'s components.
#[pyclass]
struct Vec3Iter {
    values: std::vec::IntoIter<f32>,
}

#[pymethods]
impl Vec3Iter {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(mut slf: PyRefMut<'_, Self>) -> Option<f32> {
        slf.values.next()
    }
}

// ---------------------------------------------------------------------------
// SbRotation
// ---------------------------------------------------------------------------

/// A quaternion (Coin's `SbRotation`).
#[pyclass]
#[derive(Clone, Copy, Debug)]
pub struct SbRotation {
    x: f32,
    y: f32,
    z: f32,
    w: f32,
}

impl SbRotation {
    /// Rotate `vector` by this quaternion (the `SbRotation::multVec` maths).
    fn rotate(self, vector: SbVec3f) -> SbVec3f {
        let q = [self.x, self.y, self.z];
        let v = [vector.x, vector.y, vector.z];
        // t = 2 · (q × v); v' = v + w·t + q × t
        let t = [
            2.0 * (q[1] * v[2] - q[2] * v[1]),
            2.0 * (q[2] * v[0] - q[0] * v[2]),
            2.0 * (q[0] * v[1] - q[1] * v[0]),
        ];
        SbVec3f {
            x: v[0] + self.w * t[0] + (q[1] * t[2] - q[2] * t[1]),
            y: v[1] + self.w * t[1] + (q[2] * t[0] - q[0] * t[2]),
            z: v[2] + self.w * t[2] + (q[0] * t[1] - q[1] * t[0]),
        }
    }
}

impl Default for SbRotation {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        }
    }
}

#[pymethods]
impl SbRotation {
    /// `SbRotation()`, `SbRotation(x, y, z, w)`, or `SbRotation(other)`.
    #[new]
    #[pyo3(signature = (x=0.0, y=0.0, z=0.0, w=1.0))]
    fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }

    /// `rot.getValue()` — the quaternion as `(x, y, z, w)`.
    #[pyo3(name = "getValue")]
    fn get_value(&self) -> (f32, f32, f32, f32) {
        (self.x, self.y, self.z, self.w)
    }

    /// `rot.setValue(x, y, z, w)`, or `rot.setValue((x, y, z, w))`.
    #[pyo3(name = "setValue")]
    #[pyo3(signature = (x, y=None, z=None, w=None))]
    fn set_value(
        &mut self,
        x: &Bound<'_, PyAny>,
        y: Option<f32>,
        z: Option<f32>,
        w: Option<f32>,
    ) -> PyResult<()> {
        let values: Vec<f32> = match (y, z, w) {
            (Some(y), Some(z), Some(w)) => vec![x.extract()?, y, z, w],
            _ => x.extract()?,
        };
        let [x, y, z, w]: [f32; 4] = values.try_into().map_err(|_| {
            PyValueError::new_err("SbRotation needs four components (x, y, z, w)")
        })?;
        self.x = x;
        self.y = y;
        self.z = z;
        self.w = w;
        Ok(())
    }

    /// Rotate `vector` by this rotation (`SbRotation.multVec`).
    #[pyo3(name = "multVec")]
    fn mult_vec(&self, vector: &SbVec3f) -> SbVec3f {
        self.rotate(*vector)
    }

    /// The inverse rotation.
    #[pyo3(name = "inverse")]
    fn inverse(&self) -> SbRotation {
        // Coin's inverse is the conjugate for a unit quaternion, which is all a
        // workbench builds from a placement.
        SbRotation {
            x: -self.x,
            y: -self.y,
            z: -self.z,
            w: self.w,
        }
    }

    fn __getitem__(&self, index: isize) -> PyResult<f32> {
        match index {
            0 | -4 => Ok(self.x),
            1 | -3 => Ok(self.y),
            2 | -2 => Ok(self.z),
            3 | -1 => Ok(self.w),
            _ => Err(PyIndexError::new_err("SbRotation index out of range")),
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "SbRotation({}, {}, {}, {})",
            self.x, self.y, self.z, self.w
        )
    }
}

// ---------------------------------------------------------------------------
// The camera nodes
// ---------------------------------------------------------------------------

/// The camera fields a workbench reads (and a script may set).
#[derive(Clone, Debug)]
struct CameraFields {
    position: [f32; 3],
    orientation: [f32; 4],
    focal_distance: f32,
    height: f32,
    height_angle: f32,
    aspect_ratio: f32,
    near_distance: f32,
    far_distance: f32,
    viewport_mapping: i32,
}

impl Default for CameraFields {
    fn default() -> Self {
        // Coin's own defaults: at the origin, looking down −Z, one unit away.
        Self {
            position: [0.0, 0.0, 0.0],
            orientation: [0.0, 0.0, 0.0, 1.0],
            focal_distance: 1.0,
            height: 1.0,
            height_angle: std::f32::consts::FRAC_PI_4,
            aspect_ratio: 1.0,
            near_distance: 0.1,
            far_distance: 100.0,
            viewport_mapping: 1,
        }
    }
}

/// One camera field, read and written in place (Coin's `SoSFFloat`/`SoSFVec3f`).
///
/// The state is shared with the camera node behind an `Arc<Mutex<_>>`, so a field
/// obtained from `camera.getField("position")` stays live for as long as either is
/// alive. Field *connections* are not modelled (see the module note).
#[pyclass]
pub struct SbCameraField {
    fields: Arc<Mutex<CameraFields>>,
    name: &'static str,
}

const CAMERA_FIELDS: &[&str] = &[
    "position",
    "orientation",
    "focalDistance",
    "height",
    "heightAngle",
    "aspectRatio",
    "nearDistance",
    "farDistance",
    "viewportMapping",
];

#[pymethods]
impl SbCameraField {
    /// The field's current value: an `SbVec3f`/`SbRotation`, or a number.
    #[pyo3(name = "getValue")]
    fn get_value(&self, py: Python<'_>) -> PyResult<PyObject> {
        let fields = self.fields.lock().unwrap_or_else(|e| e.into_inner());
        vector_or_number(py, &fields, self.name)
    }

    /// Set the field from an `SbVec3f`/`SbRotation`, a tuple, or a number.
    #[pyo3(name = "setValue")]
    #[pyo3(signature = (*values))]
    fn set_value(&self, values: &Bound<'_, PyTuple>) -> PyResult<()> {
        let mut fields = self.fields.lock().unwrap_or_else(|e| e.into_inner());
        set_camera_field(&mut fields, self.name, values)
    }

    fn __repr__(&self) -> String {
        format!("SbCameraField({:?})", self.name)
    }
}

/// A camera node (Coin's `SoCamera`).
#[pyclass(extends = SoNode, subclass)]
pub struct SoCamera {
    fields: Arc<Mutex<CameraFields>>,
}

impl SoCamera {
    pub(crate) fn initializer(name: &str) -> PyClassInitializer<Self> {
        node_initializer(NodeKind::Custom(name.to_string())).add_subclass(SoCamera {
            fields: Arc::new(Mutex::new(CameraFields::default())),
        })
    }

    fn field(&self, name: &'static str) -> SbCameraField {
        SbCameraField {
            fields: Arc::clone(&self.fields),
            name,
        }
    }
}

#[pymethods]
impl SoCamera {
    /// `camera.getField("position")` — the live field, or `None` for an unknown name.
    #[pyo3(name = "getField")]
    fn get_field(&self, name: &str) -> Option<SbCameraField> {
        CAMERA_FIELDS
            .iter()
            .find(|candidate| **candidate == name)
            .map(|name| self.field(name))
    }

    /// `camera.isOfType(coin.SoPerspectiveCamera.getClassTypeId())`.
    #[pyo3(name = "isOfType")]
    fn is_of_type(slf: &Bound<'_, Self>, type_id: &Bound<'_, PyAny>) -> PyResult<bool> {
        Ok(derives_from(
            slf.get_type().name()?.to_str()?,
            &type_id_name(type_id)?,
        ))
    }

    #[getter]
    fn position(&self) -> SbCameraField {
        self.field("position")
    }

    #[getter]
    fn orientation(&self) -> SbCameraField {
        self.field("orientation")
    }

    #[getter]
    #[pyo3(name = "focalDistance")]
    fn focal_distance(&self) -> SbCameraField {
        self.field("focalDistance")
    }

    #[setter]
    #[pyo3(name = "focalDistance")]
    fn set_focal_distance(&self, value: f32) {
        self.fields
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .focal_distance = value;
    }

    #[getter]
    fn height(&self) -> SbCameraField {
        self.field("height")
    }

    #[setter]
    fn set_height(&self, value: f32) {
        self.fields.lock().unwrap_or_else(|e| e.into_inner()).height = value;
    }

    #[getter]
    #[pyo3(name = "heightAngle")]
    fn height_angle(&self) -> SbCameraField {
        self.field("heightAngle")
    }

    #[setter]
    #[pyo3(name = "heightAngle")]
    fn set_height_angle(&self, value: f32) {
        self.fields
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .height_angle = value;
    }

    #[getter]
    #[pyo3(name = "aspectRatio")]
    fn aspect_ratio(&self) -> SbCameraField {
        self.field("aspectRatio")
    }

    #[setter]
    #[pyo3(name = "aspectRatio")]
    fn set_aspect_ratio(&self, value: f32) {
        self.fields
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .aspect_ratio = value;
    }

    #[getter]
    #[pyo3(name = "nearDistance")]
    fn near_distance(&self) -> SbCameraField {
        self.field("nearDistance")
    }

    #[setter]
    #[pyo3(name = "nearDistance")]
    fn set_near_distance(&self, value: f32) {
        self.fields
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .near_distance = value;
    }

    #[getter]
    #[pyo3(name = "farDistance")]
    fn far_distance(&self) -> SbCameraField {
        self.field("farDistance")
    }

    #[setter]
    #[pyo3(name = "farDistance")]
    fn set_far_distance(&self, value: f32) {
        self.fields
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .far_distance = value;
    }
}

/// A perspective camera (Coin's `SoPerspectiveCamera`).
#[pyclass(extends = SoCamera)]
pub struct SoPerspectiveCamera;

#[pymethods]
impl SoPerspectiveCamera {
    #[new]
    fn new() -> PyClassInitializer<Self> {
        SoCamera::initializer("SoPerspectiveCamera").add_subclass(SoPerspectiveCamera)
    }

    /// `coin.SoPerspectiveCamera.getClassTypeId()`.
    #[classmethod]
    #[pyo3(name = "getClassTypeId")]
    fn class_type_id(_cls: &Bound<'_, PyType>) -> SoType {
        SoType::named("SoPerspectiveCamera")
    }
}

/// An orthographic camera (Coin's `SoOrthographicCamera`).
#[pyclass(extends = SoCamera)]
pub struct SoOrthographicCamera;

#[pymethods]
impl SoOrthographicCamera {
    #[new]
    fn new() -> PyClassInitializer<Self> {
        SoCamera::initializer("SoOrthographicCamera").add_subclass(SoOrthographicCamera)
    }

    /// `coin.SoOrthographicCamera.getClassTypeId()`.
    #[classmethod]
    #[pyo3(name = "getClassTypeId")]
    fn class_type_id(_cls: &Bound<'_, PyType>) -> SoType {
        SoType::named("SoOrthographicCamera")
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The Coin type name of a type id (`SoType`, or anything with `getName()`).
fn type_id_name(type_id: &Bound<'_, PyAny>) -> PyResult<String> {
    if let Ok(name) = type_id.getattr("getName") {
        return name.call0()?.extract();
    }
    type_id.extract()
}

fn pair(
    x: &Bound<'_, PyAny>,
    y: Option<f32>,
    what: &str,
) -> PyResult<(f32, f32)> {
    let values: Vec<f32> = match y {
        Some(y) => vec![x.extract()?, y],
        None => x.extract()?,
    };
    values
        .as_slice()
        .try_into()
        .map(|values: [f32; 2]| (values[0], values[1]))
        .map_err(|_| PyValueError::new_err(format!("{what} needs two components")))
}

fn triple(
    x: &Bound<'_, PyAny>,
    y: Option<f32>,
    z: Option<f32>,
    what: &str,
) -> PyResult<(f32, f32, f32)> {
    let values: Vec<f32> = match (y, z) {
        (Some(y), Some(z)) => vec![x.extract()?, y, z],
        _ => x.extract()?,
    };
    values
        .as_slice()
        .try_into()
        .map(|values: [f32; 3]| (values[0], values[1], values[2]))
        .map_err(|_| PyValueError::new_err(format!("{what} needs three components")))
}

/// Read one camera field as the Python value Coin reports.
fn vector_or_number(
    py: Python<'_>,
    fields: &CameraFields,
    name: &str,
) -> PyResult<PyObject> {
    Ok(match name {
        "position" => Py::new(py, SbVec3f::new(
            fields.position[0],
            fields.position[1],
            fields.position[2],
        ))?
        .into_any(),
        "orientation" => Py::new(
            py,
            SbRotation::new(
                fields.orientation[0],
                fields.orientation[1],
                fields.orientation[2],
                fields.orientation[3],
            ),
        )?
        .into_any(),
        "focalDistance" => fields.focal_distance.into_py_any(py)?,
        "height" => fields.height.into_py_any(py)?,
        "heightAngle" => fields.height_angle.into_py_any(py)?,
        "aspectRatio" => fields.aspect_ratio.into_py_any(py)?,
        "nearDistance" => fields.near_distance.into_py_any(py)?,
        "farDistance" => fields.far_distance.into_py_any(py)?,
        "viewportMapping" => fields.viewport_mapping.into_py_any(py)?,
        other => {
            return Err(PyTypeError::new_err(format!(
                "unknown camera field {other:?}"
            )));
        }
    })
}

/// Write one camera field from Python values (`setValue(x, y, z)` or `setValue(vec)`).
fn set_camera_field(fields: &mut CameraFields, name: &str, values: &Bound<'_, PyTuple>) -> PyResult<()> {
    match name {
        "position" => {
            let vector = values_as_vec3(values)?;
            fields.position = [vector.x, vector.y, vector.z];
        }
        "orientation" => {
            let rotation: Vec<f32> = if values.len() == 4 {
                values.extract()?
            } else {
                values.get_item(0)?.extract()?
            };
            let [x, y, z, w]: [f32; 4] = rotation.try_into().map_err(|_| {
                PyValueError::new_err("orientation needs four components")
            })?;
            fields.orientation = [x, y, z, w];
        }
        "focalDistance" => fields.focal_distance = scalar(values)?,
        "height" => fields.height = scalar(values)?,
        "heightAngle" => fields.height_angle = scalar(values)?,
        "aspectRatio" => fields.aspect_ratio = scalar(values)?,
        "nearDistance" => fields.near_distance = scalar(values)?,
        "farDistance" => fields.far_distance = scalar(values)?,
        "viewportMapping" => {
            let value: i32 = values.get_item(0)?.extract()?;
            fields.viewport_mapping = value;
        }
        other => {
            return Err(PyTypeError::new_err(format!(
                "unknown camera field {other:?}"
            )));
        }
    }
    Ok(())
}

fn values_as_vec3(values: &Bound<'_, PyTuple>) -> PyResult<SbVec3f> {
    if values.len() == 3 {
        return Ok(SbVec3f::new(
            values.get_item(0)?.extract()?,
            values.get_item(1)?.extract()?,
            values.get_item(2)?.extract()?,
        ));
    }
    let first = values.get_item(0)?;
    if let Ok(vector) = first.downcast::<SbVec3f>() {
        return Ok(*vector.borrow());
    }
    let triple: (f32, f32, f32) = first.extract()?;
    Ok(SbVec3f::new(triple.0, triple.1, triple.2))
}

fn scalar(values: &Bound<'_, PyTuple>) -> PyResult<f32> {
    values.get_item(0)?.extract()
}

/// Register this module's classes.
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<SbVec2f>()?;
    m.add_class::<SbVec3f>()?;
    m.add_class::<SbRotation>()?;
    m.add_class::<SbCameraField>()?;
    m.add_class::<SoCamera>()?;
    m.add_class::<SoPerspectiveCamera>()?;
    m.add_class::<SoOrthographicCamera>()?;
    Ok(())
}
