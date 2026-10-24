//! `Part.ChFi2d` — the planar fillet and chamfer algorithms.
//!
//! Wraps [`ferrocad_occt::chfi2d`] (`ChFi2d_FilletAlgo`,
//! `ChFi2d_AnaFilletAlgo`, `ChFi2d_ChamferAPI`). The argument lists, overloads
//! and `TypeError`s mirror upstream's `ChFi2d_*PyImp.cpp`; results are
//! `(fillet/chamfer edge, trimmed edge 1, trimmed edge 2)` as `Part.Edge`
//! objects, so `result[0].Curve` answers the analytic curve as upstream.
//!
//! `FilletAlgo.perform()` on an uninitialised algorithm raises in OCCT; that is
//! reported as `Part.OCCError` (upstream's `Base.CADKernelError`).

use ferrocad_geom::Shape;
use ferrocad_occt::chfi2d::{AnaFilletAlgo, ChamferApi, EdgeTriple, FilletAlgo};
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::{PyModule, PyTuple};

use crate::curves::{self, EdgeEntry, OCCError};

/// The origin and normal of a `Part.Plane`.
fn plane_of(value: &Bound<'_, PyAny>) -> PyResult<([f64; 3], [f64; 3])> {
    if value.downcast::<curves::PyPlane>().is_err() {
        return Err(PyTypeError::new_err("expected a Part.Plane"));
    }
    let origin = curves::vector(&value.getattr("Location")?)?;
    let normal = curves::vector(&value.getattr("Axis")?)?;
    Ok((
        [origin.x, origin.y, origin.z],
        [normal.x, normal.y, normal.z],
    ))
}

/// A point argument (upstream's `Base::VectorPy`).
fn point_of(value: &Bound<'_, PyAny>) -> PyResult<[f64; 3]> {
    let point = curves::vector(value)
        .map_err(|_| PyTypeError::new_err("expected a Vector"))?;
    Ok([point.x, point.y, point.z])
}

/// A `Part.Wire` as a kernel shape (upstream's `TopoShapeWirePy`).
fn wire_shape(value: &Bound<'_, PyAny>) -> PyResult<Shape> {
    let wire = value
        .downcast::<curves::PyWire>()
        .map_err(|_| PyTypeError::new_err("expected a Part.Wire"))?;
    let guard = wire.borrow();
    guard
        .as_super()
        .shape()
        .ok_or_else(|| OCCError::new_err("a null Part.Wire has no geometry"))
}

/// A `Part.Edge` as a kernel shape (upstream's `TopoShapeEdgePy`).
fn edge_shape(value: &Bound<'_, PyAny>) -> PyResult<Shape> {
    let edge = value
        .downcast::<curves::PyEdge>()
        .map_err(|_| PyTypeError::new_err("expected a Part.Edge"))?;
    edge.borrow()
        .entry()
        .map(|entry| entry.shape)
        .ok_or_else(|| OCCError::new_err("an empty Part.Edge has no geometry"))
}

/// Wrap a result shape as a real `Part.Edge`, so `Curve`/`Length` work.
fn edge_object(py: Python<'_>, shape: Shape) -> PyResult<PyObject> {
    let edge = curves::PyEdge::from_entry(EdgeEntry { shape, curve: None });
    Ok(Py::new(py, edge)?.into_any())
}

/// Wrap `(new edge, trimmed edge 1, trimmed edge 2)` as a Python tuple.
fn triple_object(py: Python<'_>, triple: Option<EdgeTriple>) -> PyResult<PyObject> {
    let triple = triple.ok_or_else(|| {
        OCCError::new_err("the ChFi2d algorithm did not produce a result")
    })?;
    let mut edges = Vec::with_capacity(3);
    for shape in [triple.edge(), triple.first(), triple.second()] {
        let shape = shape.ok_or_else(|| {
            OCCError::new_err("the ChFi2d algorithm returned a null edge")
        })?;
        edges.push(edge_object(py, shape)?);
    }
    Ok(PyTuple::new(py, edges)?.into_any().unbind())
}

fn type_error(expected: &str) -> PyErr {
    PyTypeError::new_err(format!("wrong arguments; expected {expected}"))
}

/// `Part.ChFi2d.FilletAlgo` — `ChFi2d_FilletAlgo`.
#[pyclass(name = "FilletAlgo", module = "Part.ChFi2d", unsendable)]
pub struct PyFilletAlgo {
    algo: FilletAlgo,
}

#[pymethods]
impl PyFilletAlgo {
    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<Self> {
        let this = Self {
            algo: FilletAlgo::new(),
        };
        match args.len() {
            0 => Ok(this),
            2 => {
                let wire = wire_shape(&args.get_item(0)?)?;
                let (origin, normal) = plane_of(&args.get_item(1)?)?;
                this.algo.init_wire(&wire, origin, normal);
                Ok(this)
            }
            3 => {
                let edge1 = edge_shape(&args.get_item(0)?)?;
                let edge2 = edge_shape(&args.get_item(1)?)?;
                let (origin, normal) = plane_of(&args.get_item(2)?)?;
                this.algo.init_edges(&edge1, &edge2, origin, normal);
                Ok(this)
            }
            _ => Err(type_error(
                "FilletAlgo(), FilletAlgo(wire, plane) or FilletAlgo(edge, edge, plane)",
            )),
        }
    }

    #[pyo3(signature = (*args))]
    fn init(&self, args: &Bound<'_, PyTuple>) -> PyResult<()> {
        match args.len() {
            2 => {
                let wire = wire_shape(&args.get_item(0)?)?;
                let (origin, normal) = plane_of(&args.get_item(1)?)?;
                self.algo.init_wire(&wire, origin, normal);
                Ok(())
            }
            3 => {
                let edge1 = edge_shape(&args.get_item(0)?)?;
                let edge2 = edge_shape(&args.get_item(1)?)?;
                let (origin, normal) = plane_of(&args.get_item(2)?)?;
                self.algo.init_edges(&edge1, &edge2, origin, normal);
                Ok(())
            }
            _ => Err(type_error("init(wire, plane) or init(edge, edge, plane)")),
        }
    }

    fn perform(&self, radius: f64) -> PyResult<bool> {
        self.algo
            .perform(radius)
            .ok_or_else(|| OCCError::new_err("ChFi2d_FilletAlgo.perform failed"))
    }

    fn numberOfResults(&self, point: &Bound<'_, PyAny>) -> PyResult<i32> {
        let point = point_of(point)?;
        self.algo
            .number_of_results(point)
            .ok_or_else(|| OCCError::new_err("ChFi2d_FilletAlgo.numberOfResults failed"))
    }

    #[pyo3(signature = (point, solution = -1))]
    fn result(
        &self,
        py: Python<'_>,
        point: &Bound<'_, PyAny>,
        solution: i32,
    ) -> PyResult<PyObject> {
        let point = point_of(point)?;
        triple_object(py, self.algo.result(point, solution))
    }

    fn __repr__(&self) -> String {
        "<Part.ChFi2d.FilletAlgo>".to_string()
    }
}

/// `Part.ChFi2d.AnaFilletAlgo` — `ChFi2d_AnaFilletAlgo`.
#[pyclass(name = "AnaFilletAlgo", module = "Part.ChFi2d", unsendable)]
pub struct PyAnaFilletAlgo {
    algo: AnaFilletAlgo,
}

#[pymethods]
impl PyAnaFilletAlgo {
    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<Self> {
        let this = Self {
            algo: AnaFilletAlgo::new(),
        };
        match args.len() {
            0 => Ok(this),
            2 => {
                let wire = wire_shape(&args.get_item(0)?)?;
                let (origin, normal) = plane_of(&args.get_item(1)?)?;
                this.algo.init_wire(&wire, origin, normal);
                Ok(this)
            }
            3 => {
                let edge1 = edge_shape(&args.get_item(0)?)?;
                let edge2 = edge_shape(&args.get_item(1)?)?;
                let (origin, normal) = plane_of(&args.get_item(2)?)?;
                this.algo.init_edges(&edge1, &edge2, origin, normal);
                Ok(this)
            }
            _ => Err(type_error(
                "AnaFilletAlgo(), AnaFilletAlgo(wire, plane) or AnaFilletAlgo(edge, edge, plane)",
            )),
        }
    }

    #[pyo3(signature = (*args))]
    fn init(&self, args: &Bound<'_, PyTuple>) -> PyResult<()> {
        match args.len() {
            2 => {
                let wire = wire_shape(&args.get_item(0)?)?;
                let (origin, normal) = plane_of(&args.get_item(1)?)?;
                self.algo.init_wire(&wire, origin, normal);
                Ok(())
            }
            3 => {
                let edge1 = edge_shape(&args.get_item(0)?)?;
                let edge2 = edge_shape(&args.get_item(1)?)?;
                let (origin, normal) = plane_of(&args.get_item(2)?)?;
                self.algo.init_edges(&edge1, &edge2, origin, normal);
                Ok(())
            }
            _ => Err(type_error("init(wire, plane) or init(edge, edge, plane)")),
        }
    }

    fn perform(&self, radius: f64) -> bool {
        self.algo.perform(radius)
    }

    fn result(&self, py: Python<'_>) -> PyResult<PyObject> {
        triple_object(py, self.algo.result())
    }

    fn __repr__(&self) -> String {
        "<Part.ChFi2d.AnaFilletAlgo>".to_string()
    }
}

/// `Part.ChFi2d.ChamferAPI` — `ChFi2d_ChamferAPI`.
#[pyclass(name = "ChamferAPI", module = "Part.ChFi2d", unsendable)]
pub struct PyChamferApi {
    algo: ChamferApi,
}

#[pymethods]
impl PyChamferApi {
    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<Self> {
        let this = Self {
            algo: ChamferApi::new(),
        };
        match args.len() {
            1 => {
                let wire = wire_shape(&args.get_item(0)?)?;
                this.algo.init_wire(&wire);
                Ok(this)
            }
            2 => {
                let edge1 = edge_shape(&args.get_item(0)?)?;
                let edge2 = edge_shape(&args.get_item(1)?)?;
                this.algo.init_edges(&edge1, &edge2);
                Ok(this)
            }
            _ => Err(type_error("ChamferAPI(wire) or ChamferAPI(edge, edge)")),
        }
    }

    #[pyo3(signature = (*args))]
    fn init(&self, args: &Bound<'_, PyTuple>) -> PyResult<()> {
        match args.len() {
            1 => {
                let wire = wire_shape(&args.get_item(0)?)?;
                self.algo.init_wire(&wire);
                Ok(())
            }
            2 => {
                let edge1 = edge_shape(&args.get_item(0)?)?;
                let edge2 = edge_shape(&args.get_item(1)?)?;
                self.algo.init_edges(&edge1, &edge2);
                Ok(())
            }
            _ => Err(type_error("init(wire) or init(edge, edge)")),
        }
    }

    fn perform(&self) -> bool {
        self.algo.perform()
    }

    fn result(&self, py: Python<'_>, length1: f64, length2: f64) -> PyResult<PyObject> {
        triple_object(py, self.algo.result(length1, length2))
    }

    fn __repr__(&self) -> String {
        "<Part.ChFi2d.ChamferAPI>".to_string()
    }
}

/// Register `Part.ChFi2d` as a submodule of `Part`.
///
/// Upstream's `ChFi2d` reports OCCT failures as `Base.CADKernelError`; the
/// Python facade aliases that name to `Part.OCCError`, so the upstream test's
/// `assertRaises(Base.CADKernelError)` sees the class this kernel raises.
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let module = PyModule::new(m.py(), "Part.ChFi2d")?;
    module.add_class::<PyFilletAlgo>()?;
    module.add_class::<PyAnaFilletAlgo>()?;
    module.add_class::<PyChamferApi>()?;
    m.add("ChFi2d", &module)?;
    Ok(())
}
