//! `Part.Compound` and `Part.makeCompound`: compound shapes in Python.
//!
//! A compound keeps the child shapes it was built from so a later `add` can
//! rebuild the B-Rep, the same way `Part.Wire` rebuilds its wire. `Part.makeFace`
//! treats a compound as the list of its children, which is what upstream's
//! `Part.makeFace(Part.Compound(wires), ...)` expects.

use std::sync::Mutex;

use ferrocad_geom::Shape;
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::PyModule;

use crate::curves::OCCError;
use crate::PartShape;

/// `Part.Compound(shapes=None)` — a compound, a `Part.Shape` subclass.
#[pyclass(name = "Compound", module = "Part", extends = PartShape)]
pub struct PyCompound {
    children: Mutex<Vec<Shape>>,
}

impl PyCompound {
    pub(crate) fn children(&self) -> Vec<Shape> {
        self.children
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn initializer(children: Vec<Shape>) -> PyResult<PyClassInitializer<Self>> {
        let base = if children.is_empty() {
            PartShape::empty()
        } else {
            let shape = ferrocad_part::backend()
                .make_compound(&children)
                .map_err(|error| OCCError::new_err(error.to_string()))?;
            PartShape::from_shape(shape)
        };
        Ok(PyClassInitializer::from(base).add_subclass(Self {
            children: Mutex::new(children),
        }))
    }
}

#[pymethods]
impl PyCompound {
    #[new]
    #[pyo3(signature = (shapes = None))]
    fn new(shapes: Option<&Bound<'_, PyAny>>) -> PyResult<PyClassInitializer<Self>> {
        let children = match shapes {
            Some(shapes) => shapes_argument(shapes)?,
            None => Vec::new(),
        };
        Self::initializer(children)
    }

    /// Append one shape (or another compound's children) and rebuild the compound.
    fn add(mut slf: PyRefMut<'_, Self>, shape: &Bound<'_, PyAny>) -> PyResult<()> {
        let added = shapes_argument(shape)?;
        let rebuilt = {
            let mut children = slf.children.lock().unwrap_or_else(|e| e.into_inner());
            children.extend(added);
            ferrocad_part::backend()
                .make_compound(&children)
                .map_err(|error| OCCError::new_err(error.to_string()))?
        };
        slf.as_super().replace_shape(Some(rebuilt));
        Ok(())
    }

    /// The child shapes, in insertion order.
    #[getter]
    fn Shapes(&self, py: Python<'_>) -> PyResult<Vec<Py<PartShape>>> {
        self.children()
            .into_iter()
            .map(|shape| Py::new(py, PartShape::from_shape(shape)))
            .collect()
    }

    fn __len__(&self) -> usize {
        self.children
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    fn __repr__(&self) -> String {
        format!("Part.Compound({} shapes)", self.__len__())
    }
}

/// `Part.CompSolid(solids=None)` — a comp-solid, a `Part.Shape` subclass.
#[pyclass(name = "CompSolid", module = "Part", extends = PartShape)]
pub struct PyCompSolid {
    solids: Mutex<Vec<Shape>>,
}

impl PyCompSolid {
    fn initializer(solids: Vec<Shape>) -> PyResult<PyClassInitializer<Self>> {
        let base = if solids.is_empty() {
            PartShape::empty()
        } else {
            let shape = ferrocad_part::backend()
                .make_comp_solid(&solids)
                .map_err(|error| OCCError::new_err(error.to_string()))?;
            PartShape::from_shape(shape)
        };
        Ok(PyClassInitializer::from(base).add_subclass(Self {
            solids: Mutex::new(solids),
        }))
    }
}

#[pymethods]
impl PyCompSolid {
    #[new]
    #[pyo3(signature = (solids = None))]
    fn new(solids: Option<&Bound<'_, PyAny>>) -> PyResult<PyClassInitializer<Self>> {
        let solids = match solids {
            Some(solids) => shapes_argument(solids)?,
            None => Vec::new(),
        };
        Self::initializer(solids)
    }

    fn add(mut slf: PyRefMut<'_, Self>, solid: &Bound<'_, PyAny>) -> PyResult<()> {
        let added = shapes_argument(solid)?;
        let rebuilt = {
            let mut solids = slf.solids.lock().unwrap_or_else(|e| e.into_inner());
            solids.extend(added);
            ferrocad_part::backend()
                .make_comp_solid(&solids)
                .map_err(|error| OCCError::new_err(error.to_string()))?
        };
        slf.as_super().replace_shape(Some(rebuilt));
        Ok(())
    }

    fn __len__(&self) -> usize {
        self.solids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    fn __repr__(&self) -> String {
        format!("Part.CompSolid({} solids)", self.__len__())
    }
}

/// `Part.makeCompound(shapes)` — the same object as `Part.Compound(shapes)`.
#[pyfunction]
fn makeCompound(py: Python<'_>, shapes: &Bound<'_, PyAny>) -> PyResult<Py<PyCompound>> {
    Py::new(py, PyCompound::initializer(shapes_argument(shapes)?)?)
}

/// The kernel shapes named by a `Part.Shape`, a `Part.Compound`, or an iterable
/// of either.
fn shapes_argument(value: &Bound<'_, PyAny>) -> PyResult<Vec<Shape>> {
    if let Ok(compound) = value.downcast::<PyCompound>() {
        return Ok(compound.borrow().children());
    }
    if let Ok(shape) = value.downcast::<PartShape>() {
        let inner = shape
            .borrow()
            .shape()
            .ok_or_else(|| OCCError::new_err("a null shape cannot be compounded"))?;
        return Ok(vec![inner]);
    }
    if value.downcast::<pyo3::types::PyString>().is_ok() {
        return Err(PyTypeError::new_err("expected a shape or a sequence of shapes"));
    }
    let mut shapes = Vec::new();
    for item in value.try_iter()? {
        shapes.extend(shapes_argument(&item?)?);
    }
    Ok(shapes)
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyCompound>()?;
    m.add_class::<PyCompSolid>()?;
    m.add_function(wrap_pyfunction!(makeCompound, m)?)?;
    Ok(())
}
