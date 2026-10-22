//! `Part.Shell`, `Part.Solid` and the shell/solid factories in Python.
//!
//! A shell keeps the faces it was built from and a solid the shell it was built
//! from, so a later `add` can rebuild the B-Rep — the same pattern as
//! `Part.Compound` in `containers.rs`. The functional forms (`Part.makeShell`,
//! `Part.makeSolid`, `Part.makeShellFromWires`, `Part.makeLoft`,
//! `Part.makeRuledSurface`, `Part.makeFilledFace`) return a plain `Part.Shape`,
//! as upstream's `shape2pyshape` does.
//!
//! Deliberate limits for this slice, reported rather than silently ignored:
//! `makeLoft` accepts upstream's `ruled`/`closed`/`max_degree` keywords but only
//! builds the default (smoothed, open, degree-5) loft; `makeFilledFace` builds
//! the default filled face and ignores the fitting parameters upstream exposes.

use std::sync::Mutex;

use ferrocad_geom::{ElementKind, Shape};
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::PyModule;

use crate::containers::PyCompound;
use crate::curves::OCCError;
use crate::{shape_argument, PartShape};

/// Map a kernel failure to `Part.OCCError`.
fn kernel_error(error: ferrocad_geom::GeomError) -> PyErr {
    OCCError::new_err(error.to_string())
}

/// The shapes named by a `Part.Shape`, a `Part.Compound`, or an iterable of
/// either (upstream's `getPyShapes`).
fn shape_list(value: &Bound<'_, PyAny>) -> PyResult<Vec<Shape>> {
    if let Ok(compound) = value.downcast::<PyCompound>() {
        return Ok(compound.borrow().children());
    }
    if value.downcast::<PartShape>().is_ok() {
        return Ok(vec![shape_argument(value)?]);
    }
    if value.downcast::<pyo3::types::PyString>().is_ok() {
        return Err(PyTypeError::new_err(
            "expected a shape or a sequence of shapes",
        ));
    }
    let mut shapes = Vec::new();
    for item in value.try_iter()? {
        shapes.extend(shape_list(&item?)?);
    }
    Ok(shapes)
}

/// The shells of a shape: itself when it is a shell, otherwise its shells.
fn shells_of(shape: &Shape) -> Vec<Shape> {
    ferrocad_part::backend()
        .elements(shape, ElementKind::Shell)
        .into_iter()
        .map(|element| element.shape)
        .collect()
}

/// `Part.Shell(faces=None)` — a shell, a `Part.Shape` subclass.
#[pyclass(name = "Shell", module = "Part", extends = PartShape)]
pub struct PyShell {
    faces: Mutex<Vec<Shape>>,
}

impl PyShell {
    fn initializer(faces: Vec<Shape>) -> PyResult<PyClassInitializer<Self>> {
        let base = if faces.is_empty() {
            PartShape::empty()
        } else {
            let shape = ferrocad_part::backend()
                .make_shell(&faces)
                .map_err(kernel_error)?;
            PartShape::from_shape(shape)
        };
        Ok(PyClassInitializer::from(base).add_subclass(Self {
            faces: Mutex::new(faces),
        }))
    }
}

#[pymethods]
impl PyShell {
    /// `Part.Shell()` is the null shell; `Part.Shell(faces)` builds a shell from
    /// those faces (upstream's constructor with a list of shapes).
    #[new]
    #[pyo3(signature = (shapes = None))]
    fn new(shapes: Option<&Bound<'_, PyAny>>) -> PyResult<PyClassInitializer<Self>> {
        let faces = match shapes {
            Some(shapes) => shape_list(shapes)?,
            None => Vec::new(),
        };
        Self::initializer(faces)
    }

    /// Append a face and rebuild the shell.
    fn add(mut slf: PyRefMut<'_, Self>, face: &Bound<'_, PyAny>) -> PyResult<()> {
        let added = shape_list(face)?;
        let rebuilt = {
            let mut faces = slf.faces.lock().unwrap_or_else(|e| e.into_inner());
            faces.extend(added);
            ferrocad_part::backend()
                .make_shell(&faces)
                .map_err(kernel_error)?
        };
        slf.as_super().replace_shape(Some(rebuilt));
        Ok(())
    }

    fn __len__(&self) -> usize {
        self.faces
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    fn __repr__(&self) -> String {
        format!("Part.Shell({} faces)", self.__len__())
    }
}

/// `Part.Solid(shell=None)` — a solid, a `Part.Shape` subclass.
#[pyclass(name = "Solid", module = "Part", extends = PartShape)]
pub struct PySolid {
    shells: Mutex<Vec<Shape>>,
}

impl PySolid {
    fn initializer(shells: Vec<Shape>) -> PyResult<PyClassInitializer<Self>> {
        let base = if shells.is_empty() {
            PartShape::empty()
        } else {
            let shape = solid_from(&shells)?;
            PartShape::from_shape(shape)
        };
        Ok(PyClassInitializer::from(base).add_subclass(Self {
            shells: Mutex::new(shells),
        }))
    }
}

/// Build a solid from the collected shells.
///
/// The seam's `make_solid` takes one closed shell (this slice has no cavities), so
/// more than one shell is reported instead of silently dropping the rest.
fn solid_from(shells: &[Shape]) -> PyResult<Shape> {
    match shells {
        [shell] => ferrocad_part::backend()
            .make_solid(shell)
            .map_err(kernel_error),
        [] => Err(OCCError::new_err("a solid needs a shell")),
        _ => Err(OCCError::new_err(
            "a solid from several shells (cavities) is not implemented yet",
        )),
    }
}

#[pymethods]
impl PySolid {
    /// `Part.Solid()` is the null solid; `Part.Solid(shape)` builds a solid from
    /// the shells of `shape` (upstream's constructor).
    #[new]
    #[pyo3(signature = (shape = None))]
    fn new(shape: Option<&Bound<'_, PyAny>>) -> PyResult<PyClassInitializer<Self>> {
        let shells = match shape {
            Some(shape) => shells_of(&shape_argument(shape)?),
            None => Vec::new(),
        };
        Self::initializer(shells)
    }

    /// Append a shell (or the shells of a solid) and rebuild the solid.
    fn add(mut slf: PyRefMut<'_, Self>, shape: &Bound<'_, PyAny>) -> PyResult<()> {
        let added = shells_of(&shape_argument(shape)?);
        if added.is_empty() {
            return Err(OCCError::new_err("Part.Solid.add expects a shell"));
        }
        // Build from the extended list before committing it, so a rejected add
        // (a second shell) leaves the object as it was.
        let rebuilt = {
            let mut shells = slf.shells.lock().unwrap_or_else(|e| e.into_inner());
            let mut candidate = shells.clone();
            candidate.extend(added);
            let shape = solid_from(&candidate)?;
            *shells = candidate;
            shape
        };
        slf.as_super().replace_shape(Some(rebuilt));
        Ok(())
    }

    fn __len__(&self) -> usize {
        self.shells
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    fn __repr__(&self) -> String {
        format!("Part.Solid({} shells)", self.__len__())
    }
}

/// `Part.makeShell(faces, op=None)` — create a shell out of a list of faces.
#[pyfunction]
#[pyo3(signature = (shapes, op = None))]
fn makeShell(shapes: &Bound<'_, PyAny>, op: Option<&str>) -> PyResult<PartShape> {
    let _ = op;
    let shape = ferrocad_part::backend()
        .make_shell(&shape_list(shapes)?)
        .map_err(kernel_error)?;
    Ok(PartShape::from_shape(shape))
}

/// `Part.makeShellFromWires(shape, op=None)` — create a shell from boundary
/// wires (upstream names the keyword `shape`, although it is a list of wires).
///
/// Each wire becomes one planar face; the wires must bound faces that together
/// form an open or closed shell.
#[pyfunction]
#[pyo3(signature = (shape, op = None))]
fn makeShellFromWires(shape: &Bound<'_, PyAny>, op: Option<&str>) -> PyResult<PartShape> {
    let _ = op;
    let shell = ferrocad_part::backend()
        .make_shell_from_wires(&shape_list(shape)?)
        .map_err(kernel_error)?;
    Ok(PartShape::from_shape(shell))
}

/// `Part.makeSolid(shape, op=None)` — create a solid from the shell of `shape`.
///
/// A shell is used directly; any other shape must contain exactly one shell.
#[pyfunction]
#[pyo3(signature = (shape, op = None))]
fn makeSolid(shape: &Bound<'_, PyAny>, op: Option<&str>) -> PyResult<PartShape> {
    let _ = op;
    let shell = shape_argument(shape)?;
    let solid = solid_from(&shells_of(&shell))?;
    Ok(PartShape::from_shape(solid))
}

/// `Part.makeLoft(shapes, solid=False, ruled=False, closed=False, max_degree=5, op=None)`.
///
/// Lofts through the section wires in the given order. `ruled`, `closed` and
/// `max_degree` are accepted for upstream compatibility; this slice builds the
/// default smoothed, open, degree-5 loft.
#[pyfunction]
#[pyo3(signature = (shapes, solid = false, ruled = false, closed = false, max_degree = 5, op = None))]
fn makeLoft(
    shapes: &Bound<'_, PyAny>,
    solid: bool,
    ruled: bool,
    closed: bool,
    max_degree: i32,
    op: Option<&str>,
) -> PyResult<PartShape> {
    let _ = (ruled, closed, max_degree, op);
    let lofts = ferrocad_part::backend()
        .make_loft(&shape_list(shapes)?, solid)
        .map_err(kernel_error)?;
    Ok(PartShape::from_shape(lofts))
}

/// `Part.makeRuledSurface(edge1, edge2)` — a ruled surface between two edges.
#[pyfunction]
#[pyo3(signature = (edge1, edge2))]
fn makeRuledSurface(edge1: &Bound<'_, PyAny>, edge2: &Bound<'_, PyAny>) -> PyResult<PartShape> {
    let first = shape_argument(edge1)?;
    let second = shape_argument(edge2)?;
    let surface = ferrocad_part::backend()
        .make_ruled_surface(&first, &second)
        .map_err(kernel_error)?;
    Ok(PartShape::from_shape(surface))
}

/// `Part.makeFilledFace(edges, op=None)` — create a face out of a list of edges.
///
/// The edges are joined into one wire and filled with the kernel's filling
/// surface. Upstream's optional fitting parameters are not implemented.
#[pyfunction]
#[pyo3(signature = (shapes, op = None))]
fn makeFilledFace(shapes: &Bound<'_, PyAny>, op: Option<&str>) -> PyResult<PartShape> {
    let _ = op;
    let face = ferrocad_part::backend()
        .make_filled_face(&shape_list(shapes)?)
        .map_err(kernel_error)?;
    Ok(PartShape::from_shape(face))
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyShell>()?;
    m.add_class::<PySolid>()?;
    m.add_function(wrap_pyfunction!(makeShell, m)?)?;
    m.add_function(wrap_pyfunction!(makeShellFromWires, m)?)?;
    m.add_function(wrap_pyfunction!(makeSolid, m)?)?;
    m.add_function(wrap_pyfunction!(makeLoft, m)?)?;
    m.add_function(wrap_pyfunction!(makeRuledSurface, m)?)?;
    m.add_function(wrap_pyfunction!(makeFilledFace, m)?)?;
    Ok(())
}
