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

use ferrocad_inventor::{
    Node, NodeKind, ShaderParameter, ShaderStage, ShaderStageKind, Topology, depth_function,
    light_model, pick_style,
};
use ferrocad_types::{Placement, Rotation, Vector3};
use pyo3::exceptions::{PyIndexError, PyRuntimeError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyList, PyTuple};
use pyo3::{IntoPyObjectExt, PyClass};

use crate::{SoNode, node_initializer, node_snapshot_initializer};

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
    /// `SoFaceSet.numVertices`.
    NumVertices,
    /// `SoShaderParameterArray1f.value` (`SoMFFloat`).
    ParameterValues,
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
            (NodeKind::FaceSet { num_vertices }, FieldKind::NumVertices) => Ok(num_vertices),
            _ => Err(mismatch(Self::LABEL)),
        }
    }
}

impl FieldElem for f32 {
    const LABEL: &'static str = "SoMFFloat";

    fn to_py(self, py: Python<'_>) -> PyResult<PyObject> {
        f64::from(self).into_py_any(py)
    }

    fn from_py(obj: &Bound<'_, PyAny>) -> PyResult<Self> {
        obj.extract()
    }

    fn buffer<'a>(kind: &'a mut NodeKind, field: FieldKind) -> PyResult<&'a mut Vec<Self>> {
        match (kind, field) {
            (
                NodeKind::ShaderParameter(ShaderParameter::Floats { values, .. }),
                FieldKind::ParameterValues,
            ) => Ok(values),
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

/// `SoMField.setValues(start, num, values)` — Coin's three-argument overload.
///
/// Replaces `num` elements from `start`, ignoring any surplus values in `values`;
/// the field grows (filling any gap with the element default) when needed. Fewer
/// values than `num` are written when the sequence is short; the elements past
/// the written run are left untouched, unlike the two-argument form.
fn field_set_values_range<E: FieldElem>(
    parent: &Py<PyAny>,
    field: FieldKind,
    py: Python<'_>,
    start: usize,
    num: i64,
    values: &Bound<'_, PyAny>,
) -> PyResult<()> {
    if num < 0 {
        return Err(PyValueError::new_err(format!(
            "setValues: num must be non-negative, got {num}"
        )));
    }
    let parsed = parse_values::<E>(values)?;
    let count = (num as usize).min(parsed.len());
    if count == 0 {
        return Ok(());
    }
    let end = start
        .checked_add(count)
        .ok_or_else(|| PyValueError::new_err("setValues: start + num overflows"))?;
    let mut node = node_mut(py, parent)?;
    let buffer = E::buffer(&mut node.node.kind, field)?;
    if buffer.len() < end {
        buffer.resize(end, E::default());
    }
    buffer[start..end].copy_from_slice(&parsed[..count]);
    Ok(())
}

/// `SoMField.setNum(n)` — resize to `n`, filling new elements with the default.
fn field_set_num<E: FieldElem>(
    parent: &Py<PyAny>,
    field: FieldKind,
    py: Python<'_>,
    num: i64,
) -> PyResult<()> {
    if num < 0 {
        return Err(PyValueError::new_err(format!(
            "setNum: num must be non-negative, got {num}"
        )));
    }
    let mut node = node_mut(py, parent)?;
    let buffer = E::buffer(&mut node.node.kind, field)?;
    buffer.resize(num as usize, E::default());
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

            /// `setValues(offset, values)` or Coin's `setValues(start, num, values)`.
            #[pyo3(name = "setValues", signature = (start, num, values = None))]
            fn set_values(
                &self,
                py: Python<'_>,
                start: usize,
                num: &Bound<'_, PyAny>,
                values: Option<&Bound<'_, PyAny>>,
            ) -> PyResult<()> {
                match values {
                    Some(values) => {
                        let num = num.extract::<i64>().map_err(|_| {
                            PyTypeError::new_err(
                                "setValues(start, num, values): num must be an integer",
                            )
                        })?;
                        field_set_values_range::<$elem>(
                            &self.parent,
                            self.field,
                            py,
                            start,
                            num,
                            values,
                        )
                    }
                    None => field_set_values::<$elem>(&self.parent, self.field, py, start, num),
                }
            }

            /// `field.setNum(n)` — grow with the element default or truncate.
            #[pyo3(name = "setNum")]
            fn set_num(&self, py: Python<'_>, num: i64) -> PyResult<()> {
                field_set_num::<$elem>(&self.parent, self.field, py, num)
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
multi_field!(SoMFFloat, f32);

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

/// A non-indexed polygon set drawn from the current `Coordinate3`
/// (Coin `SoFaceSet`).
///
/// `numVertices` gives one vertex count per polygon; the compiler consumes that
/// many consecutive coordinates per face and fan-triangulates it.
#[pyclass(extends = SoNode)]
pub struct SoFaceSet;

#[pymethods]
impl SoFaceSet {
    #[new]
    pub fn new() -> PyClassInitializer<Self> {
        node_initializer(NodeKind::FaceSet {
            num_vertices: Vec::new(),
        })
        .add_subclass(SoFaceSet)
    }

    #[getter]
    #[pyo3(name = "numVertices")]
    fn num_vertices(slf: &Bound<'_, Self>) -> PyResult<Py<SoMFInt32>> {
        Py::new(
            slf.py(),
            SoMFInt32 {
                parent: field_parent(slf),
                field: FieldKind::NumVertices,
            },
        )
    }

    #[setter]
    #[pyo3(name = "numVertices")]
    fn set_num_vertices(mut slf: PyRefMut<'_, Self>, values: &Bound<'_, PyAny>) -> PyResult<()> {
        let parsed = parse_values::<u32>(values)?;
        field_replace(
            &mut slf.as_super().node.kind,
            FieldKind::NumVertices,
            parsed,
        )
    }
}

/// A pickability hint (Coin `SoPickStyle`).
///
/// **Recorded, not applied**: the compiler traverses past it and keeps drawing
/// the subtree. The pickability semantics arrive with the pipeline wave.
#[pyclass(extends = SoNode)]
pub struct SoPickStyle;

#[pymethods]
impl SoPickStyle {
    #[new]
    pub fn new() -> PyClassInitializer<Self> {
        node_initializer(NodeKind::PickStyle {
            style: pick_style::SHAPE,
        })
        .add_subclass(SoPickStyle)
    }

    #[classattr]
    const SHAPE: i32 = pick_style::SHAPE;
    #[classattr]
    const BOUNDING_BOX: i32 = pick_style::BOUNDING_BOX;
    #[classattr]
    const UNPICKABLE: i32 = pick_style::UNPICKABLE;

    #[getter]
    fn style(slf: PyRef<'_, Self>) -> i32 {
        match &slf.as_super().node.kind {
            NodeKind::PickStyle { style } => *style,
            _ => pick_style::SHAPE,
        }
    }

    #[setter]
    fn set_style(mut slf: PyRefMut<'_, Self>, value: i32) -> PyResult<()> {
        match &mut slf.as_super().node.kind {
            NodeKind::PickStyle { style } => {
                *style = value;
                Ok(())
            }
            _ => Err(PyRuntimeError::new_err("SoPickStyle: not a pick style")),
        }
    }
}

/// A shading-model hint (Coin `SoLightModel`).
///
/// **Recorded, not applied**: under a shader program the shader owns shading and
/// without one FerroCAD has a single light model.
#[pyclass(extends = SoNode)]
pub struct SoLightModel;

#[pymethods]
impl SoLightModel {
    #[new]
    pub fn new() -> PyClassInitializer<Self> {
        node_initializer(NodeKind::LightModel {
            model: light_model::PHONG,
        })
        .add_subclass(SoLightModel)
    }

    #[classattr]
    const BASE_COLOR: i32 = light_model::BASE_COLOR;
    #[classattr]
    const PHONG: i32 = light_model::PHONG;

    #[getter]
    fn model(slf: PyRef<'_, Self>) -> i32 {
        match &slf.as_super().node.kind {
            NodeKind::LightModel { model } => *model,
            _ => light_model::PHONG,
        }
    }

    #[setter]
    fn set_model(mut slf: PyRefMut<'_, Self>, value: i32) -> PyResult<()> {
        match &mut slf.as_super().node.kind {
            NodeKind::LightModel { model } => {
                *model = value;
                Ok(())
            }
            _ => Err(PyRuntimeError::new_err("SoLightModel: not a light model")),
        }
    }
}

/// Depth-buffer state (Coin `SoDepthBuffer`).
///
/// **Recorded, not applied**: the renderer gains the third pipeline variant (test
/// on, write off) in a later wave. The default `function` is `LESS`, matching the
/// renderer's current opaque pipeline; Coin's default is not verified locally.
#[pyclass(extends = SoNode)]
pub struct SoDepthBuffer;

#[pymethods]
impl SoDepthBuffer {
    #[new]
    pub fn new() -> PyClassInitializer<Self> {
        node_initializer(NodeKind::DepthBuffer {
            test: true,
            write: true,
            function: depth_function::LESS,
        })
        .add_subclass(SoDepthBuffer)
    }

    #[classattr]
    const NEVER: i32 = depth_function::NEVER;
    #[classattr]
    const LESS: i32 = depth_function::LESS;
    #[classattr]
    const EQUAL: i32 = depth_function::EQUAL;
    #[classattr]
    const LEQUAL: i32 = depth_function::LEQUAL;
    #[classattr]
    const GREATER: i32 = depth_function::GREATER;
    #[classattr]
    const NOTEQUAL: i32 = depth_function::NOTEQUAL;
    #[classattr]
    const GEQUAL: i32 = depth_function::GEQUAL;
    #[classattr]
    const ALWAYS: i32 = depth_function::ALWAYS;

    #[getter]
    fn test(slf: PyRef<'_, Self>) -> bool {
        match &slf.as_super().node.kind {
            NodeKind::DepthBuffer { test, .. } => *test,
            _ => true,
        }
    }

    #[setter]
    fn set_test(mut slf: PyRefMut<'_, Self>, value: bool) -> PyResult<()> {
        match &mut slf.as_super().node.kind {
            NodeKind::DepthBuffer { test, .. } => {
                *test = value;
                Ok(())
            }
            _ => Err(PyRuntimeError::new_err("SoDepthBuffer: not a depth buffer")),
        }
    }

    #[getter]
    fn write(slf: PyRef<'_, Self>) -> bool {
        match &slf.as_super().node.kind {
            NodeKind::DepthBuffer { write, .. } => *write,
            _ => true,
        }
    }

    #[setter]
    fn set_write(mut slf: PyRefMut<'_, Self>, value: bool) -> PyResult<()> {
        match &mut slf.as_super().node.kind {
            NodeKind::DepthBuffer { write, .. } => {
                *write = value;
                Ok(())
            }
            _ => Err(PyRuntimeError::new_err("SoDepthBuffer: not a depth buffer")),
        }
    }

    #[getter]
    fn function(slf: PyRef<'_, Self>) -> i32 {
        match &slf.as_super().node.kind {
            NodeKind::DepthBuffer { function, .. } => *function,
            _ => depth_function::LESS,
        }
    }

    #[setter]
    fn set_function(mut slf: PyRefMut<'_, Self>, value: i32) -> PyResult<()> {
        match &mut slf.as_super().node.kind {
            NodeKind::DepthBuffer { function, .. } => {
                *function = value;
                Ok(())
            }
            _ => Err(PyRuntimeError::new_err("SoDepthBuffer: not a depth buffer")),
        }
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

// ---------------------------------------------------------------------------
// `SoMFNode`: a list of node references
// ---------------------------------------------------------------------------

/// Which node-reference list a [`SoMFNode`] view addresses.
#[derive(Clone, Copy, Debug)]
pub(crate) enum NodeListField {
    /// `SoShaderProgram.shaderObject`.
    ShaderObject,
    /// A shader stage's `parameter` list.
    Parameter,
}

impl NodeListField {
    fn matches(self, kind: &NodeKind) -> bool {
        matches!(
            (self, kind),
            (Self::ShaderObject, NodeKind::ShaderProgram)
                | (Self::Parameter, NodeKind::ShaderStage(_))
        )
    }

    fn label(self) -> &'static str {
        match self {
            Self::ShaderObject => "SoShaderProgram.shaderObject",
            Self::Parameter => "shader stage.parameter",
        }
    }
}

/// Borrow the node list a [`SoMFNode`] view addresses, verifying the parent is the
/// kind of node that owns it.
fn node_list_mut<'py>(
    py: Python<'py>,
    parent: &Py<PyAny>,
    field: NodeListField,
) -> PyResult<PyRefMut<'py, SoNode>> {
    let node = node_mut(py, parent)?;
    if !field.matches(&node.node.kind) {
        return Err(mismatch(field.label()));
    }
    Ok(node)
}

/// Copy a Python node's backing node, as `addChild` does.
fn node_from_py(obj: &Bound<'_, PyAny>) -> PyResult<Node> {
    let node = obj
        .downcast::<SoNode>()
        .map_err(|_| PyTypeError::new_err("SoMFNode expects `coin` scene-graph nodes"))?;
    Ok(node
        .try_borrow()
        .map_err(|_| PyRuntimeError::new_err("SoMFNode: the node is already in use"))?
        .node
        .clone())
}

/// Parse a Python iterable of nodes.
fn parse_nodes(values: &Bound<'_, PyAny>) -> PyResult<Vec<Node>> {
    let mut parsed = Vec::new();
    for item in values.try_iter()? {
        parsed.push(node_from_py(&item?)?);
    }
    Ok(parsed)
}

/// A detached snapshot of a stored node, for reads.
fn detached_node(py: Python<'_>, node: &Node) -> PyResult<PyObject> {
    Ok(Py::new(py, node_snapshot_initializer(node.clone()))?.into_any())
}

/// A live Coin `SoMFNode`: a list of node references (`SoShaderProgram.shaderObject`,
/// a shader stage's `parameter`).
///
/// **Value semantics.** `addChild` copies a node into its parent, so this field copies
/// too: assignment appends or replaces a snapshot of the given node, and a later edit
/// to the caller's node does not reach the field (the value-semantics choice in
/// `docs/grid-overlay-plan.md` §7.2). Reads return detached `SoNode` snapshots for the
/// same reason — Coin's pointer identity is not available.
///
/// The element list is the parent's [`Node::children`], which is exactly a node
/// reference list; the program node's stage list and the stage's parameter list are
/// both stored there. Writing `setValues` past the end is an error (there is no null
/// node to fill the gap), unlike the numeric fields.
#[pyclass]
pub struct SoMFNode {
    parent: Py<PyAny>,
    field: NodeListField,
}

#[pymethods]
impl SoMFNode {
    fn __len__(&self, py: Python<'_>) -> PyResult<usize> {
        Ok(node_list_mut(py, &self.parent, self.field)?
            .node
            .children
            .len())
    }

    /// `field.getNum()`.
    #[pyo3(name = "getNum")]
    fn get_num(&self, py: Python<'_>) -> PyResult<usize> {
        self.__len__(py)
    }

    /// `field[i]` — a detached snapshot of the stored node.
    fn __getitem__(&self, py: Python<'_>, index: isize) -> PyResult<PyObject> {
        let node = node_list_mut(py, &self.parent, self.field)?;
        let index = resolve_index(index, node.node.children.len())?;
        detached_node(py, &node.node.children[index])
    }

    fn __setitem__(&self, py: Python<'_>, index: isize, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let replacement = node_from_py(value)?;
        let mut node = node_list_mut(py, &self.parent, self.field)?;
        let index = resolve_index(index, node.node.children.len())?;
        node.node.children[index] = replacement;
        Ok(())
    }

    fn __delitem__(&self, py: Python<'_>, index: isize) -> PyResult<()> {
        let mut node = node_list_mut(py, &self.parent, self.field)?;
        let index = resolve_index(index, node.node.children.len())?;
        node.node.children.remove(index);
        Ok(())
    }

    /// `field.getValues()` — detached snapshots of every stored node.
    #[pyo3(name = "getValues")]
    fn get_values(&self, py: Python<'_>) -> PyResult<PyObject> {
        let node = node_list_mut(py, &self.parent, self.field)?;
        let list = PyList::empty(py);
        for child in &node.node.children {
            list.append(detached_node(py, child)?)?;
        }
        Ok(list.into_any().unbind())
    }

    /// `setValues(offset, values)` or Coin's `setValues(start, num, values)`.
    #[pyo3(name = "setValues", signature = (start, num, values = None))]
    fn set_values(
        &self,
        py: Python<'_>,
        start: usize,
        num: &Bound<'_, PyAny>,
        values: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let Some(values) = values else {
            let parsed = parse_nodes(num)?;
            let mut node = node_list_mut(py, &self.parent, self.field)?;
            if start > node.node.children.len() {
                return Err(past_end(start, node.node.children.len()));
            }
            node.node.children.truncate(start);
            node.node.children.extend(parsed);
            return Ok(());
        };
        let num = num.extract::<i64>().map_err(|_| {
            PyTypeError::new_err("setValues(start, num, values): num must be an integer")
        })?;
        if num < 0 {
            return Err(PyValueError::new_err(format!(
                "setValues: num must be non-negative, got {num}"
            )));
        }
        let parsed = parse_nodes(values)?;
        let count = (num as usize).min(parsed.len());
        let mut node = node_list_mut(py, &self.parent, self.field)?;
        let list = &mut node.node.children;
        if start > list.len() {
            return Err(past_end(start, list.len()));
        }
        let overwritten = count.min(list.len() - start);
        list[start..start + overwritten].clone_from_slice(&parsed[..overwritten]);
        list.extend_from_slice(&parsed[overwritten..count]);
        Ok(())
    }

    /// `field.setValue(node)` — clear the list and store one node.
    #[pyo3(name = "setValue")]
    fn set_value(&self, py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let parsed = node_from_py(value)?;
        let mut node = node_list_mut(py, &self.parent, self.field)?;
        node.node.children.clear();
        node.node.children.push(parsed);
        Ok(())
    }

    /// `field.set1Value(index, node)` — replace one slot; `index == len` appends.
    #[pyo3(name = "set1Value")]
    fn set_1_value(&self, py: Python<'_>, index: usize, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let parsed = node_from_py(value)?;
        let mut node = node_list_mut(py, &self.parent, self.field)?;
        let list = &mut node.node.children;
        if index > list.len() {
            return Err(past_end(index, list.len()));
        }
        if index == list.len() {
            list.push(parsed);
        } else {
            list[index] = parsed;
        }
        Ok(())
    }

    /// `field.append(node)` — Coin's `SoMFNode` has no append; this is a convenience.
    #[pyo3(name = "append")]
    fn append(&self, py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let parsed = node_from_py(value)?;
        node_list_mut(py, &self.parent, self.field)?
            .node
            .children
            .push(parsed);
        Ok(())
    }

    fn __repr__(&self, py: Python<'_>) -> String {
        match node_list_mut(py, &self.parent, self.field) {
            Ok(node) => format!("SoMFNode (len {})", node.node.children.len()),
            Err(_) => "SoMFNode (detached)".to_owned(),
        }
    }
}

fn past_end(index: usize, len: usize) -> PyErr {
    PyIndexError::new_err(format!(
        "SoMFNode: index {index} is past the end (len {len})"
    ))
}

// ---------------------------------------------------------------------------
// Shader value nodes
// ---------------------------------------------------------------------------

/// A shader program (Coin `SoShaderProgram`).
///
/// `shaderObject` (`SoMFNode`) links one vertex and one fragment stage. The program
/// draws nothing itself: an active program binds to the **following siblings** in the
/// enclosing list. Under the fixed WGSL ABI the Coin stage split is compatibility-only;
/// the module is the concatenation of the stages' non-empty `source`s.
#[pyclass(extends = SoNode)]
pub struct SoShaderProgram;

#[pymethods]
impl SoShaderProgram {
    #[new]
    pub fn new() -> PyClassInitializer<Self> {
        node_initializer(NodeKind::ShaderProgram).add_subclass(SoShaderProgram)
    }

    /// `program.shaderObject` — the linked stages.
    #[getter]
    #[pyo3(name = "shaderObject")]
    fn shader_object(slf: &Bound<'_, Self>) -> PyResult<Py<SoMFNode>> {
        Py::new(
            slf.py(),
            SoMFNode {
                parent: field_parent(slf),
                field: NodeListField::ShaderObject,
            },
        )
    }
}

fn stage_of(kind: &NodeKind, expected: ShaderStageKind) -> PyResult<&ShaderStage> {
    match kind {
        NodeKind::ShaderStage(stage) if stage.kind == expected => Ok(stage),
        _ => Err(PyRuntimeError::new_err(format!(
            "{}: not a shader stage",
            expected.class_name()
        ))),
    }
}

fn stage_of_mut(kind: &mut NodeKind, expected: ShaderStageKind) -> PyResult<&mut ShaderStage> {
    match kind {
        NodeKind::ShaderStage(stage) if stage.kind == expected => Ok(stage),
        _ => Err(PyRuntimeError::new_err(format!(
            "{}: not a shader stage",
            expected.class_name()
        ))),
    }
}

/// Emit one stage class (`SoVertexShader`/`SoFragmentShader`).
///
/// The two differ only in their `ShaderStageKind`, so the getters/setters are shared
/// here. `source` is a FerroCAD extension (inline WGSL, authoritative when set);
/// `sourceProgram` is Coin's path form and needs an explicit base directory at
/// extraction time.
macro_rules! shader_stage_class {
    ($name:ident, $kind:expr, $doc:literal) => {
        #[doc = $doc]
        #[pyclass(extends = SoNode)]
        pub struct $name;

        #[pymethods]
        impl $name {
            #[new]
            pub fn new() -> PyClassInitializer<Self> {
                node_initializer(NodeKind::ShaderStage(ShaderStage {
                    kind: $kind,
                    ..ShaderStage::default()
                }))
                .add_subclass($name)
            }

            /// `source` — inline WGSL (FerroCAD extension; authoritative when set).
            #[getter]
            fn source(slf: PyRef<'_, Self>) -> PyResult<String> {
                Ok(stage_of(&slf.as_super().node.kind, $kind)?
                    .source
                    .clone()
                    .unwrap_or_default())
            }

            #[setter]
            fn set_source(mut slf: PyRefMut<'_, Self>, value: &str) -> PyResult<()> {
                let stage = stage_of_mut(&mut slf.as_super().node.kind, $kind)?;
                stage.source = (!value.is_empty()).then(|| value.to_owned());
                Ok(())
            }

            /// `sourceProgram` — Coin's file path (compatibility-only).
            #[getter]
            #[pyo3(name = "sourceProgram")]
            fn source_program(slf: PyRef<'_, Self>) -> PyResult<String> {
                Ok(stage_of(&slf.as_super().node.kind, $kind)?
                    .source_program
                    .clone()
                    .unwrap_or_default())
            }

            #[setter]
            #[pyo3(name = "sourceProgram")]
            fn set_source_program(mut slf: PyRefMut<'_, Self>, value: &str) -> PyResult<()> {
                let stage = stage_of_mut(&mut slf.as_super().node.kind, $kind)?;
                stage.source_program = (!value.is_empty()).then(|| value.to_owned());
                Ok(())
            }

            /// `stage.parameter` — the fixed-ABI parameter nodes (`SoMFNode`).
            #[getter]
            fn parameter(slf: &Bound<'_, Self>) -> PyResult<Py<SoMFNode>> {
                Py::new(
                    slf.py(),
                    SoMFNode {
                        parent: field_parent(slf),
                        field: NodeListField::Parameter,
                    },
                )
            }
        }
    };
}

shader_stage_class!(
    SoVertexShader,
    ShaderStageKind::Vertex,
    "A vertex shader stage (Coin `SoVertexShader`)."
);
shader_stage_class!(
    SoFragmentShader,
    ShaderStageKind::Fragment,
    "A fragment shader stage (Coin `SoFragmentShader`)."
);

fn parameter_name(kind: &NodeKind) -> PyResult<&str> {
    match kind {
        NodeKind::ShaderParameter(parameter) => Ok(parameter.name()),
        _ => Err(PyRuntimeError::new_err("not a shader parameter")),
    }
}

fn set_parameter_name(kind: &mut NodeKind, name: &str) -> PyResult<()> {
    match kind {
        NodeKind::ShaderParameter(
            ShaderParameter::Float { name: slot, .. }
            | ShaderParameter::Vec3 { name: slot, .. }
            | ShaderParameter::Floats { name: slot, .. },
        ) => {
            *slot = name.to_owned();
            Ok(())
        }
        _ => Err(PyRuntimeError::new_err("not a shader parameter")),
    }
}

/// Emit the shared `name` accessor pair for a shader parameter class.
// The pair has to live inside each `#[pymethods]` block (pyo3 resolves `#[getter]`
// before an inner macro would expand), so the classes spell it out below.

/// A float parameter (Coin `SoShaderParameter1f`).
#[pyclass(extends = SoNode)]
pub struct SoShaderParameter1f;

#[pymethods]
impl SoShaderParameter1f {
    #[new]
    pub fn new() -> PyClassInitializer<Self> {
        node_initializer(NodeKind::ShaderParameter(ShaderParameter::Float {
            name: String::new(),
            value: 0.0,
        }))
        .add_subclass(SoShaderParameter1f)
    }

    #[getter]
    fn name(slf: PyRef<'_, Self>) -> PyResult<String> {
        Ok(parameter_name(&slf.as_super().node.kind)?.to_owned())
    }

    #[setter]
    fn set_name(mut slf: PyRefMut<'_, Self>, value: &str) -> PyResult<()> {
        set_parameter_name(&mut slf.as_super().node.kind, value)
    }

    #[getter]
    fn value(slf: PyRef<'_, Self>) -> PyResult<f32> {
        match &slf.as_super().node.kind {
            NodeKind::ShaderParameter(ShaderParameter::Float { value, .. }) => Ok(*value),
            _ => Err(PyRuntimeError::new_err("not a SoShaderParameter1f")),
        }
    }

    #[setter]
    fn set_value(mut slf: PyRefMut<'_, Self>, value: f32) -> PyResult<()> {
        match &mut slf.as_super().node.kind {
            NodeKind::ShaderParameter(ShaderParameter::Float { value: slot, .. }) => {
                *slot = value;
                Ok(())
            }
            _ => Err(PyRuntimeError::new_err("not a SoShaderParameter1f")),
        }
    }
}

/// A vec3 parameter (Coin `SoShaderParameter3f`).
#[pyclass(extends = SoNode)]
pub struct SoShaderParameter3f;

#[pymethods]
impl SoShaderParameter3f {
    #[new]
    pub fn new() -> PyClassInitializer<Self> {
        node_initializer(NodeKind::ShaderParameter(ShaderParameter::Vec3 {
            name: String::new(),
            value: [0.0; 3],
        }))
        .add_subclass(SoShaderParameter3f)
    }

    #[getter]
    fn name(slf: PyRef<'_, Self>) -> PyResult<String> {
        Ok(parameter_name(&slf.as_super().node.kind)?.to_owned())
    }

    #[setter]
    fn set_name(mut slf: PyRefMut<'_, Self>, value: &str) -> PyResult<()> {
        set_parameter_name(&mut slf.as_super().node.kind, value)
    }

    #[getter]
    fn value(slf: PyRef<'_, Self>) -> PyResult<(f64, f64, f64)> {
        match &slf.as_super().node.kind {
            NodeKind::ShaderParameter(ShaderParameter::Vec3 { value, .. }) => Ok((
                f64::from(value[0]),
                f64::from(value[1]),
                f64::from(value[2]),
            )),
            _ => Err(PyRuntimeError::new_err("not a SoShaderParameter3f")),
        }
    }

    #[setter]
    fn set_value(mut slf: PyRefMut<'_, Self>, value: (f32, f32, f32)) -> PyResult<()> {
        match &mut slf.as_super().node.kind {
            NodeKind::ShaderParameter(ShaderParameter::Vec3 { value: slot, .. }) => {
                *slot = [value.0, value.1, value.2];
                Ok(())
            }
            _ => Err(PyRuntimeError::new_err("not a SoShaderParameter3f")),
        }
    }
}

/// A float-array parameter (Coin `SoShaderParameterArray1f`, `value` is `SoMFFloat`).
#[pyclass(extends = SoNode)]
pub struct SoShaderParameterArray1f;

#[pymethods]
impl SoShaderParameterArray1f {
    #[new]
    pub fn new() -> PyClassInitializer<Self> {
        node_initializer(NodeKind::ShaderParameter(ShaderParameter::Floats {
            name: String::new(),
            values: Vec::new(),
        }))
        .add_subclass(SoShaderParameterArray1f)
    }

    #[getter]
    fn name(slf: PyRef<'_, Self>) -> PyResult<String> {
        Ok(parameter_name(&slf.as_super().node.kind)?.to_owned())
    }

    #[setter]
    fn set_name(mut slf: PyRefMut<'_, Self>, value: &str) -> PyResult<()> {
        set_parameter_name(&mut slf.as_super().node.kind, value)
    }

    /// `value` — the live `SoMFFloat` field.
    #[getter]
    fn value(slf: &Bound<'_, Self>) -> PyResult<Py<SoMFFloat>> {
        Py::new(
            slf.py(),
            SoMFFloat {
                parent: field_parent(slf),
                field: FieldKind::ParameterValues,
            },
        )
    }

    #[setter]
    fn set_value(mut slf: PyRefMut<'_, Self>, values: &Bound<'_, PyAny>) -> PyResult<()> {
        let parsed = parse_values::<f32>(values)?;
        match &mut slf.as_super().node.kind {
            NodeKind::ShaderParameter(ShaderParameter::Floats { values, .. }) => {
                *values = parsed;
                Ok(())
            }
            _ => Err(PyRuntimeError::new_err("not a SoShaderParameterArray1f")),
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
    m.add_class::<SoFaceSet>()?;
    m.add_class::<SoPickStyle>()?;
    m.add_class::<SoLightModel>()?;
    m.add_class::<SoDepthBuffer>()?;
    m.add_class::<SoTransform>()?;
    m.add_class::<SoMFVec3f>()?;
    m.add_class::<SoMFColor>()?;
    m.add_class::<SoMFInt32>()?;
    m.add_class::<SoMFFloat>()?;
    m.add_class::<SoMFNode>()?;
    m.add_class::<SoShaderProgram>()?;
    m.add_class::<SoVertexShader>()?;
    m.add_class::<SoFragmentShader>()?;
    m.add_class::<SoShaderParameter1f>()?;
    m.add_class::<SoShaderParameter3f>()?;
    m.add_class::<SoShaderParameterArray1f>()?;
    Ok(())
}
