//! `Part.Compound` and `Part.makeCompound`: compound shapes in Python.
//!
//! A compound keeps the child shapes it was built from so a later `add` can
//! rebuild the B-Rep, the same way `Part.Wire` rebuilds its wire. `Part.makeFace`
//! treats a compound as the list of its children, which is what upstream's
//! `Part.makeFace(Part.Compound(wires), ...)` expects.

use std::sync::Mutex;

use ferrocad_geom::{ElementKind, Shape};
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::PyModule;

use crate::curves::OCCError;
use crate::PartShape;

/// `Part.Compound(shapes=None)` — a compound, a `Part.Shape` subclass.
#[pyclass(name = "Compound", module = "Part", extends = PartShape)]
pub struct PyCompound {
    children: Mutex<Vec<Shape>>,
    /// The base shape's [`PartShape::epoch`] the cache was taken at.
    cached_at: Mutex<u64>,
}

impl PyCompound {
    /// The child shapes, in insertion order.
    ///
    /// The cache is re-derived from the compound's current B-Rep whenever the
    /// base shape changed through the inherited API (`Shape.rotate`,
    /// `Shape.transformShape`, …). Without that check a rotated compound kept
    /// reporting its children's pre-rotation geometry — which is what
    /// `Part.makeCompound([rotated])` would then rebuild from.
    pub(crate) fn children(&self, base: &PartShape) -> Vec<Shape> {
        self.children_at(base.epoch(), base.shape())
    }

    /// [`children`](Self::children) for a base reached through a `PyRefMut`,
    /// where the base cannot be borrowed at the same time as the subclass.
    pub(crate) fn children_at(&self, epoch: u64, shape: Option<Shape>) -> Vec<Shape> {
        let mut cached_at = self.cached_at.lock().unwrap_or_else(|e| e.into_inner());
        if *cached_at == epoch {
            drop(cached_at);
            return self
                .children
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
        }
        let children = direct_children(shape.as_ref());
        *self
            .children
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = children.clone();
        *cached_at = epoch;
        children
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
        let epoch = base.epoch();
        Ok(PyClassInitializer::from(base).add_subclass(Self {
            children: Mutex::new(children),
            cached_at: Mutex::new(epoch),
        }))
    }
}

/// The direct sub-shapes of `base`, highest dimension first.
///
/// A compound's children have no kernel query of their own, so this walks the
/// element lists and takes the first kind that is present — the same inference
/// `Shape.SubShapes` makes. A compound that mixes dimensions (a solid and an
/// edge, say) therefore reports only its highest-dimension children.
fn direct_children(shape: Option<&Shape>) -> Vec<Shape> {
    let Some(shape) = shape else {
        return Vec::new();
    };
    let backend = ferrocad_part::backend();
    for kind in [
        ElementKind::Solid,
        ElementKind::Shell,
        ElementKind::Face,
        ElementKind::Wire,
        ElementKind::Edge,
        ElementKind::Vertex,
    ] {
        let elements = backend.elements(shape, kind);
        if !elements.is_empty() {
            return elements.into_iter().map(|element| element.shape).collect();
        }
    }
    Vec::new()
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
        let (epoch, current) = {
            let base = slf.as_super();
            (base.epoch(), base.shape())
        };
        let mut children = PyCompound::children_at(&slf, epoch, current);
        children.extend(added);
        let rebuilt = ferrocad_part::backend()
            .make_compound(&children)
            .map_err(|error| OCCError::new_err(error.to_string()))?;
        slf.as_super().replace_shape(Some(rebuilt));
        *slf.children.lock().unwrap_or_else(|e| e.into_inner()) = children;
        *slf.cached_at.lock().unwrap_or_else(|e| e.into_inner()) = slf.as_super().epoch();
        Ok(())
    }

    /// The child shapes, in insertion order.
    #[getter]
    fn Shapes(slf: PyRef<'_, Self>, py: Python<'_>) -> PyResult<Vec<Py<PartShape>>> {
        slf.children(slf.as_super())
            .into_iter()
            .map(|shape| Py::new(py, PartShape::from_shape(shape)))
            .collect()
    }

    fn __len__(slf: PyRef<'_, Self>) -> usize {
        slf.children(slf.as_super()).len()
    }

    fn __repr__(slf: PyRef<'_, Self>) -> String {
        format!("Part.Compound({} shapes)", slf.children(slf.as_super()).len())
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
pub(crate) fn shapes_argument(value: &Bound<'_, PyAny>) -> PyResult<Vec<Shape>> {
    if let Ok(compound) = value.downcast::<PyCompound>() {
        let compound = compound.borrow();
        return Ok(compound.children(compound.as_super()));
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
