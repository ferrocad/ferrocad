//! Geometry field objects over the owned node model.
//!
//! A workbench does not assign whole lists to a node — it mutates fields in place
//! (`coords.point[0] = (x, y, z)`, `faces.coordIndex.setValues(0, idx)`), so the
//! getters here return **live field objects** that read and write the wrapped
//! [`NodeKind`] directly. Each is the sequence-protocol surface of a Coin
//! `SoMField` (`__len__`, `__getitem__` → a Python tuple, `__setitem__`,
//! `getValues`/`setValues`/`setValue`/`set1Value`/`getNum`).
//!
//! NumPy zero-copy ingestion, detached-field RAII and generational dirty counters
//! are deferred (see `docs/inventor-plan.md` §8); editing a field just rewrites the
//! node's `Vec`.

use ferrocad_inventor::{NodeKind, Topology};
use ferrocad_types::{Placement, Rotation, Vector3};
use pyo3::exceptions::{PyIndexError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyList, PyTuple};
use pyo3::{IntoPyObjectExt, PyClass};

use crate::{node_initializer, SoNode};

// ---------------------------------------------------------------------------
// Field selection
// ---------------------------------------------------------------------------

/// Which buffer inside a node's [`NodeKind`] a field object addresses.
#[derive(Clone, Copy, Debug)]
pub(crate) enum FieldKind {
    /// `SoCoordinate3.point`.
    Point,
    /// `SoNormal.vector`.
    Vector,
    /// `SoMaterial.diffuseColor`.
    DiffuseColor,
    /// `SoIndexedShape.coordIndex`.
    CoordIndex,
    /// `SoIndexedFaceSet.normalIndex`.
    NormalIndex,
}

/// A multi-value element: how to convert it to/from Python and which node buffer
/// holds it. The sequence helpers below are generic over this.
trait FieldElem: Copy + Default {
    /// The Coin field class name (for errors and `repr`).
    const LABEL: &'static str;

    /// Convert one element to a Python object.
    fn to_py(self, py: Python<'_>) -> PyResult<PyObject>;

    /// Parse one element from a Python object.
    fn from_py(obj: &Bound<'_, PyAny>) -> PyResult<Self>;

    /// The node buffer this element type backs for `field`.
    fn buffer<'a>(kind: &'a mut NodeKind, field: FieldKind) -> PyResult<&'a mut Vec<Self>>;
}

impl FieldElem for [f32; 3] {
    const LABEL: &'static str = "SoMFVec3f";

    fn to_py(self, py: Python<'_>) -> PyResult<PyObject> {
        Ok(PyTuple::new(py, [f64::from(self[0]), f64::from(self[1]), f64::from(self[2])])?
            .into_any()
            .unbind())
    }

    fn from_py(obj: &Bound<'_, PyAny>) -> PyResult<Self> {
        let values: Vec<f32> = obj.extract()?;
        match values.as_slice() {
            [x, y, z] => Ok([*x, *y, *z]),
            other => Err(PyValueError::new_err(format!(
                "SoMFVec3f expects 3 components, got {}",
                other.len()
            ))),
        }
    }

    fn buffer<'a>(kind: &'a mut NodeKind, field: FieldKind) -> PyResult<&'a mut Vec<Self>> {
        match (kind, field) {
            (NodeKind::Coordinate3(v), FieldKind::Point) => Ok(v),
            (NodeKind::Normal(v), FieldKind::Vector) => Ok(v),
            _ => Err(mismatch(Self::LABEL)),
        }
    }
}

impl FieldElem for [f32; 4] {
    const LABEL: &'static str = "SoMFColor";

    fn to_py(self, py: Python<'_>) -> PyResult<PyObject> {
        Ok(PyTuple::new(
            py,
            [
                f64::from(self[0]),
                f64::from(self[1]),
                f64::from(self[2]),
                f64::from(self[3]),
            ],
        )?
        .into_any()
        .unbind())
    }

    fn from_py(obj: &Bound<'_, PyAny>) -> PyResult<Self> {
        // Coin accepts RGB (alpha defaults to 1) as well as RGBA.
        let values: Vec<f32> = obj.extract()?;
        match values.as_slice() {
            [r, g, b] => Ok([*r, *g, *b, 1.0]),
            [r, g, b, a] => Ok([*r, *g, *b, *a]),
            other => Err(PyValueError::new_err(format!(
                "SoMFColor expects 3 or 4 components, got {}",
                other.len()
            ))),
        }
    }

    fn buffer<'a>(kind: &'a mut NodeKind, field: FieldKind) -> PyResult<&'a mut Vec<Self>> {
        match (kind, field) {
            (NodeKind::Material(v), FieldKind::DiffuseColor) => Ok(v),
            _ => Err(mismatch(Self::LABEL)),
        }
    }
}

impl FieldElem for u32 {
    const LABEL: &'static str = "SoMFInt32";

    fn to_py(self, py: Python<'_>) -> PyResult<PyObject> {
        self.into_py_any(py)
    }

    fn from_py(obj: &Bound<'_, PyAny>) -> PyResult<Self> {
        let value: i64 = obj.extract()?;
        u32::try_from(value).map_err(|_| {
            PyValueError::new_err(format!(
                "SoMFInt32 index must be non-negative, got {value} \
                 (polygon delimiters like -1 are not supported yet)"
            ))
        })
    }

    fn buffer<'a>(kind: &'a mut NodeKind, field: FieldKind) -> PyResult<&'a mut Vec<Self>> {
        match (kind, field) {
            (NodeKind::Mesh { indices, .. }, FieldKind::CoordIndex) => Ok(indices),
            (NodeKind::Mesh { normal_indices, .. }, FieldKind::NormalIndex) => Ok(normal_indices),
            _ => Err(mismatch(Self::LABEL)),
        }
    }
}

fn mismatch(label: &str) -> PyErr {
    PyRuntimeError::new_err(format!("{label}: the parent node has no such field"))
}

// ---------------------------------------------------------------------------
// The sequence helpers (generic over the element type)
// ---------------------------------------------------------------------------

/// Borrow the node a field object belongs to (exclusively).
fn node_mut<'py>(py: Python<'py>, parent: &Py<PyAny>) -> PyResult<PyRefMut<'py, SoNode>> {
    parent
        .bind(py)
        .downcast::<SoNode>()
        .map_err(|_| PyRuntimeError::new_err("field parent is not a scene node"))?
        .try_borrow_mut()
        .map_err(|_| PyRuntimeError::new_err("the scene node is already in use"))
}

/// Resolve a possibly-negative index against a length, as Python sequences do.
fn resolve_index(index: isize, len: usize) -> PyResult<usize> {
    let signed = len as isize;
    let resolved = if index < 0 { index + signed } else { index };
    if resolved < 0 || resolved >= signed {
        Err(PyIndexError::new_err("field index out of range"))
    } else {
        Ok(resolved as usize)
    }
}

fn field_len<E: FieldElem>(parent: &Py<PyAny>, field: FieldKind, py: Python<'_>) -> PyResult<usize> {
    let mut node = node_mut(py, parent)?;
    Ok(E::buffer(&mut node.node.kind, field)?.len())
}

fn field_get<E: FieldElem>(
    parent: &Py<PyAny>,
    field: FieldKind,
    py: Python<'_>,
    index: isize,
) -> PyResult<PyObject> {
    let mut node = node_mut(py, parent)?;
    let buffer = E::buffer(&mut node.node.kind, field)?;
    let index = resolve_index(index, buffer.len())?;
    buffer[index].to_py(py)
}

fn field_set<E: FieldElem>(
    parent: &Py<PyAny>,
    field: FieldKind,
    py: Python<'_>,
    index: isize,
    value: &Bound<'_, PyAny>,
) -> PyResult<()> {
    let parsed = E::from_py(value)?;
    let mut node = node_mut(py, parent)?;
    let buffer = E::buffer(&mut node.node.kind, field)?;
    let index = resolve_index(index, buffer.len())?;
    buffer[index] = parsed;
    Ok(())
}

fn field_del<E: FieldElem>(
    parent: &Py<PyAny>,
    field: FieldKind,
    py: Python<'_>,
    index: isize,
) -> PyResult<()> {
    let mut node = node_mut(py, parent)?;
    let buffer = E::buffer(&mut node.node.kind, field)?;
    let index = resolve_index(index, buffer.len())?;
    buffer.remove(index);
    Ok(())
}

fn field_values<E: FieldElem>(
    parent: &Py<PyAny>,
    field: FieldKind,
    py: Python<'_>,
) -> PyResult<PyObject> {
    let mut node = node_mut(py, parent)?;
    let buffer = E::buffer(&mut node.node.kind, field)?;
    let list = PyList::empty(py);
    for value in buffer.iter() {
        list.append(value.to_py(py)?)?;
    }
    Ok(list.into_any().unbind())
}

/// Parse any Python iterable into a vector of elements.
fn parse_values<E: FieldElem>(values: &Bound<'_, PyAny>) -> PyResult<Vec<E>> {
    let mut parsed = Vec::new();
    for item in values.try_iter()? {
        parsed.push(E::from_py(&item?)?);
    }
    Ok(parsed)
}

/// Replace a node buffer wholesale (the property setter, e.g. `coords.point = [...]`).
fn field_replace<E: FieldElem>(kind: &mut NodeKind, field: FieldKind, values: Vec<E>) -> PyResult<()> {
    *E::buffer(kind, field)? = values;
    Ok(())
}

/// `SoMField.setValues(offset, values)` — Coin replaces the run from `offset`.
fn field_set_values<E: FieldElem>(
    parent: &Py<PyAny>,
    field: FieldKind,
    py: Python<'_>,
    offset: usize,
    values: &Bound<'_, PyAny>,
) -> PyResult<()> {
    let parsed = parse_values::<E>(values)?;
    let mut node = node_mut(py, parent)?;
    let buffer = E::buffer(&mut node.node.kind, field)?;
    if offset > buffer.len() {
        buffer.resize(offset, E::default());
    }
    buffer.truncate(offset);
    buffer.extend_from_slice(&parsed);
    Ok(())
}

/// `SoMField.setValue(value)` — the singular overload: a one-element field.
fn field_set_value<E: FieldElem>(
    parent: &Py<PyAny>,
    field: FieldKind,
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
) -> PyResult<()> {
    let parsed = E::from_py(value)?;
    let mut node = node_mut(py, parent)?;
    let buffer = E::buffer(&mut node.node.kind, field)?;
    buffer.clear();
    buffer.push(parsed);
    Ok(())
}

/// `SoMField.set1Value(index, value)` — set one element, growing to reach it.
fn field_set_1_value<E: FieldElem>(
    parent: &Py<PyAny>,
    field: FieldKind,
    py: Python<'_>,
    index: usize,
    value: &Bound<'_, PyAny>,
) -> PyResult<()> {
    let parsed = E::from_py(value)?;
    let mut node = node_mut(py, parent)?;
    let buffer = E::buffer(&mut node.node.kind, field)?;
    if buffer.len() <= index {
        buffer.resize(index + 1, E::default());
    }
    buffer[index] = parsed;
    Ok(())
}

// ---------------------------------------------------------------------------
// The field classes
// ---------------------------------------------------------------------------

/// Generate a `SoMField`-style container class over one [`FieldElem`].
macro_rules! multi_field {
    ($name:ident, $elem:ty) => {
        /// A live Coin `SoMField`-style field (sequence protocol over a node buffer).
        #[pyclass]
        pub struct $name {
            parent: Py<PyAny>,
            field: FieldKind,
        }

        #[pymethods]
        impl $name {
            fn __len__(&self, py: Python<'_>) -> PyResult<usize> {
                field_len::<$elem>(&self.parent, self.field, py)
            }

            /// `field.getNum()`.
            #[pyo3(name = "getNum")]
            fn get_num(&self, py: Python<'_>) -> PyResult<usize> {
                field_len::<$elem>(&self.parent, self.field, py)
            }

            fn __getitem__(&self, py: Python<'_>, index: isize) -> PyResult<PyObject> {
                field_get::<$elem>(&self.parent, self.field, py, index)
            }

            fn __setitem__(&self, py: Python<'_>, index: isize, value: &Bound<'_, PyAny>) -> PyResult<()> {
                field_set::<$elem>(&self.parent, self.field, py, index, value)
            }

            fn __delitem__(&self, py: Python<'_>, index: isize) -> PyResult<()> {
                field_del::<$elem>(&self.parent, self.field, py, index)
            }

            /// `field.getValues()` — the whole buffer as a Python list.
            #[pyo3(name = "getValues")]
            fn get_values(&self, py: Python<'_>) -> PyResult<PyObject> {
                field_values::<$elem>(&self.parent, self.field, py)
            }

            #[pyo3(name = "setValues")]
            fn set_values(&self, py: Python<'_>, offset: usize, values: &Bound<'_, PyAny>) -> PyResult<()> {
                field_set_values::<$elem>(&self.parent, self.field, py, offset, values)
            }

            #[pyo3(name = "setValue")]
            fn set_value(&self, py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<()> {
                field_set_value::<$elem>(&self.parent, self.field, py, value)
            }

            #[pyo3(name = "set1Value")]
            fn set_1_value(&self, py: Python<'_>, index: usize, value: &Bound<'_, PyAny>) -> PyResult<()> {
                field_set_1_value::<$elem>(&self.parent, self.field, py, index, value)
            }

            fn __repr__(&self, py: Python<'_>) -> String {
                match field_len::<$elem>(&self.parent, self.field, py) {
                    Ok(len) => format!("{} (len {})", <$elem as FieldElem>::LABEL, len),
                    Err(_) => format!("{} (detached)", <$elem as FieldElem>::LABEL),
                }
            }
        }
    };
}

multi_field!(SoMFVec3f, [f32; 3]);
multi_field!(SoMFColor, [f32; 4]);
multi_field!(SoMFInt32, u32);

/// The owning Python object, kept alive for the field's lifetime.
fn field_parent<T: PyClass>(slf: &Bound<'_, T>) -> Py<PyAny> {
    slf.clone().into_any().unbind()
}

// ---------------------------------------------------------------------------
// Geometry / transform nodes
// ---------------------------------------------------------------------------

/// Vertex positions for the following shape (Coin `SoCoordinate3`).
#[pyclass(extends = SoNode)]
pub struct SoCoordinate3;

#[pymethods]
impl SoCoordinate3 {
    #[new]
    pub fn new() -> PyClassInitializer<Self> {
        node_initializer(NodeKind::Coordinate3(Vec::new())).add_subclass(SoCoordinate3)
    }

    #[getter]
    fn point(slf: &Bound<'_, Self>) -> PyResult<Py<SoMFVec3f>> {
        Py::new(
            slf.py(),
            SoMFVec3f {
                parent: field_parent(slf),
                field: FieldKind::Point,
            },
        )
    }

    #[setter]
    fn set_point(mut slf: PyRefMut<'_, Self>, values: &Bound<'_, PyAny>) -> PyResult<()> {
        let parsed = parse_values::<[f32; 3]>(values)?;
        field_replace(&mut slf.as_super().node.kind, FieldKind::Point, parsed)
    }
}

/// Vertex normals for the following shape (Coin `SoNormal`).
#[pyclass(extends = SoNode)]
pub struct SoNormal;

#[pymethods]
impl SoNormal {
    #[new]
    pub fn new() -> PyClassInitializer<Self> {
        node_initializer(NodeKind::Normal(Vec::new())).add_subclass(SoNormal)
    }

    #[getter]
    fn vector(slf: &Bound<'_, Self>) -> PyResult<Py<SoMFVec3f>> {
        Py::new(
            slf.py(),
            SoMFVec3f {
                parent: field_parent(slf),
                field: FieldKind::Vector,
            },
        )
    }

    #[setter]
    fn set_vector(mut slf: PyRefMut<'_, Self>, values: &Bound<'_, PyAny>) -> PyResult<()> {
        let parsed = parse_values::<[f32; 3]>(values)?;
        field_replace(&mut slf.as_super().node.kind, FieldKind::Vector, parsed)
    }
}

/// The colours for the following shapes (Coin `SoMaterial`).
#[pyclass(extends = SoNode)]
pub struct SoMaterial;

#[pymethods]
impl SoMaterial {
    #[new]
    pub fn new() -> PyClassInitializer<Self> {
        node_initializer(NodeKind::Material(Vec::new())).add_subclass(SoMaterial)
    }

    #[getter]
    #[pyo3(name = "diffuseColor")]
    fn diffuse_color(slf: &Bound<'_, Self>) -> PyResult<Py<SoMFColor>> {
        Py::new(
            slf.py(),
            SoMFColor {
                parent: field_parent(slf),
                field: FieldKind::DiffuseColor,
            },
        )
    }

    #[setter]
    #[pyo3(name = "diffuseColor")]
    fn set_diffuse_color(mut slf: PyRefMut<'_, Self>, values: &Bound<'_, PyAny>) -> PyResult<()> {
        let parsed = parse_values::<[f32; 4]>(values)?;
        field_replace(&mut slf.as_super().node.kind, FieldKind::DiffuseColor, parsed)
    }
}

/// An indexed triangle set drawn from the current `Coordinate3`
/// (Coin `SoIndexedFaceSet`).
#[pyclass(extends = SoNode)]
pub struct SoIndexedFaceSet;

#[pymethods]
impl SoIndexedFaceSet {
    #[new]
    pub fn new() -> PyClassInitializer<Self> {
        node_initializer(NodeKind::Mesh {
            indices: Vec::new(),
            normal_indices: Vec::new(),
            topology: Topology::Triangles,
        })
        .add_subclass(SoIndexedFaceSet)
    }

    #[getter]
    #[pyo3(name = "coordIndex")]
    fn coord_index(slf: &Bound<'_, Self>) -> PyResult<Py<SoMFInt32>> {
        Py::new(
            slf.py(),
            SoMFInt32 {
                parent: field_parent(slf),
                field: FieldKind::CoordIndex,
            },
        )
    }

    #[setter]
    #[pyo3(name = "coordIndex")]
    fn set_coord_index(mut slf: PyRefMut<'_, Self>, values: &Bound<'_, PyAny>) -> PyResult<()> {
        let parsed = parse_values::<u32>(values)?;
        field_replace(&mut slf.as_super().node.kind, FieldKind::CoordIndex, parsed)
    }

    #[getter]
    #[pyo3(name = "normalIndex")]
    fn normal_index(slf: &Bound<'_, Self>) -> PyResult<Py<SoMFInt32>> {
        Py::new(
            slf.py(),
            SoMFInt32 {
                parent: field_parent(slf),
                field: FieldKind::NormalIndex,
            },
        )
    }

    #[setter]
    #[pyo3(name = "normalIndex")]
    fn set_normal_index(mut slf: PyRefMut<'_, Self>, values: &Bound<'_, PyAny>) -> PyResult<()> {
        let parsed = parse_values::<u32>(values)?;
        field_replace(&mut slf.as_super().node.kind, FieldKind::NormalIndex, parsed)
    }
}

/// An indexed line set drawn from the current `Coordinate3`
/// (Coin `SoIndexedLineSet`).
#[pyclass(extends = SoNode)]
pub struct SoIndexedLineSet;

#[pymethods]
impl SoIndexedLineSet {
    #[new]
    pub fn new() -> PyClassInitializer<Self> {
        node_initializer(NodeKind::Mesh {
            indices: Vec::new(),
            normal_indices: Vec::new(),
            topology: Topology::Lines,
        })
        .add_subclass(SoIndexedLineSet)
    }

    #[getter]
    #[pyo3(name = "coordIndex")]
    fn coord_index(slf: &Bound<'_, Self>) -> PyResult<Py<SoMFInt32>> {
        Py::new(
            slf.py(),
            SoMFInt32 {
                parent: field_parent(slf),
                field: FieldKind::CoordIndex,
            },
        )
    }

    #[setter]
    #[pyo3(name = "coordIndex")]
    fn set_coord_index(mut slf: PyRefMut<'_, Self>, values: &Bound<'_, PyAny>) -> PyResult<()> {
        let parsed = parse_values::<u32>(values)?;
        field_replace(&mut slf.as_super().node.kind, FieldKind::CoordIndex, parsed)
    }
}

/// A transform for the following siblings (Coin `SoTransform`).
///
/// Exposes `translation` (a 3-tuple) and `rotation` (an axis-angle 4-tuple,
/// `(x, y, z, radians)`). The singular `SoSFVec3f`/`SoSFRotation` field objects
/// (with `.setValue`) are a later refinement.
#[pyclass(extends = SoNode)]
pub struct SoTransform;

#[pymethods]
impl SoTransform {
    #[new]
    pub fn new() -> PyClassInitializer<Self> {
        node_initializer(NodeKind::Transform(Placement::identity())).add_subclass(SoTransform)
    }

    #[getter]
    fn translation(slf: PyRef<'_, Self>) -> PyResult<(f64, f64, f64)> {
        match &slf.as_super().node.kind {
            NodeKind::Transform(placement) => {
                Ok((placement.base.x, placement.base.y, placement.base.z))
            }
            _ => Err(PyRuntimeError::new_err("SoTransform: not a transform")),
        }
    }

    #[setter]
    fn set_translation(mut slf: PyRefMut<'_, Self>, value: (f64, f64, f64)) -> PyResult<()> {
        match &mut slf.as_super().node.kind {
            NodeKind::Transform(placement) => {
                placement.base = Vector3::new(value.0, value.1, value.2);
                Ok(())
            }
            _ => Err(PyRuntimeError::new_err("SoTransform: not a transform")),
        }
    }

    #[getter]
    fn rotation(slf: PyRef<'_, Self>) -> PyResult<(f64, f64, f64, f64)> {
        match &slf.as_super().node.kind {
            NodeKind::Transform(placement) => {
                let axis = placement.rotation.axis();
                Ok((axis.x, axis.y, axis.z, placement.rotation.angle()))
            }
            _ => Err(PyRuntimeError::new_err("SoTransform: not a transform")),
        }
    }

    #[setter]
    fn set_rotation(mut slf: PyRefMut<'_, Self>, value: (f64, f64, f64, f64)) -> PyResult<()> {
        match &mut slf.as_super().node.kind {
            NodeKind::Transform(placement) => {
                placement.rotation =
                    Rotation::from_axis_angle(&Vector3::new(value.0, value.1, value.2), value.3);
                Ok(())
            }
            _ => Err(PyRuntimeError::new_err("SoTransform: not a transform")),
        }
    }
}

/// Register every class this module defines (called from the `coin` module).
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<SoCoordinate3>()?;
    m.add_class::<SoNormal>()?;
    m.add_class::<SoMaterial>()?;
    m.add_class::<SoIndexedFaceSet>()?;
    m.add_class::<SoIndexedLineSet>()?;
    m.add_class::<SoTransform>()?;
    m.add_class::<SoMFVec3f>()?;
    m.add_class::<SoMFColor>()?;
    m.add_class::<SoMFInt32>()?;
    Ok(())
}
