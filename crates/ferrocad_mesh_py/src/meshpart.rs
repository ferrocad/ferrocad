//! `MeshPart`: the Part → Mesh bridge.
//!
//! `meshFromShape` calls the duck-typed shape's `tessellate(LinearDeflection,
//! AngularDeflection)` binding (this slice does **not** import `Part` in Rust),
//! and `wireFromSegment` asks the mesh kernel for the boundary loops of a facet
//! group and builds `Part.Wire`s through `Part.makePolygon` in Python.

use ferrocad_mesh::Mesh as CoreMesh;
use ferrocad_py::PyVector;
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyList, PyModule, PyTuple};

use crate::{PyMesh, index_triple, mesh_error, point, py_point};

/// The tessellation weld: `gp::Resolution()`, the tolerance upstream's
/// `Mesher::Vertex` uses for its `std::set`.
const WELD_TOLERANCE: f64 = 1e-7;

/// `MeshPart.meshFromShape(Shape, LinearDeflection=0.5, AngularDeflection=0.5,
/// Relative=False, Segments=False)` → `Mesh.Mesh`.
///
/// `Shape` is duck-typed: any object with an upstream-compatible
/// `tessellate(LinearDeflection, AngularDeflection)` returning
/// `(points, facets)` works (this Python module never imports `Part`). The
/// result is already welded by the tessellator, so its point and facet order is
/// preserved and the mesh placement starts at the identity.
///
/// `Relative` is accepted and ignored (the tessellator is always absolute in
/// this slice).
///
/// With `Segments=True` each shape **face** (`Shape.Faces`) is tessellated on
/// its own and the per-face parts are welded together within
/// [`WELD_TOLERANCE`], exactly like upstream's `Mesher` (one facet group per
/// face, not one per connected region). A duck-typed shape without a `Faces`
/// attribute falls back to one whole-shape tessellation in a single group.
#[pyfunction]
#[pyo3(signature = (Shape, LinearDeflection = 0.5, AngularDeflection = 0.5, Relative = false, Segments = false))]
fn meshFromShape(
    Shape: &Bound<'_, PyAny>,
    LinearDeflection: f64,
    AngularDeflection: f64,
    Relative: bool,
    Segments: bool,
) -> PyResult<PyMesh> {
    let _ = Relative;

    if Segments {
        let parts = match Shape.getattr("Faces") {
            Ok(faces) => faces
                .try_iter()?
                .map(|face| {
                    let face = face?;
                    tessellate(&face, LinearDeflection, AngularDeflection)
                })
                .collect::<PyResult<Vec<_>>>()?,
            Err(_) => vec![tessellate(Shape, LinearDeflection, AngularDeflection)?],
        };
        return CoreMesh::from_segmented_facets(&parts, WELD_TOLERANCE)
            .map(PyMesh::from_core)
            .map_err(mesh_error);
    }

    let (points, facets) = tessellate(Shape, LinearDeflection, AngularDeflection)?;
    CoreMesh::new(points, facets)
        .map(PyMesh::from_core)
        .map_err(mesh_error)
}

/// Read a shape's `(points, facets)` tessellation.
///
/// The upstream signature takes the linear deflection first and the angular one
/// second (radians), which is the order the binding already exposes.
fn tessellate(
    shape: &Bound<'_, PyAny>,
    linear_deflection: f64,
    angular_deflection: f64,
) -> PyResult<(Vec<ferrocad_types::Vector3>, Vec<[u32; 3]>)> {
    let tessellation =
        shape.call_method1("tessellate", (linear_deflection, angular_deflection))?;
    let tessellation = tessellation.downcast::<PyTuple>().map_err(|_| {
        PyTypeError::new_err("Shape.tessellate() must return a (points, facets) tuple")
    })?;
    if tessellation.len() != 2 {
        return Err(PyTypeError::new_err(
            "Shape.tessellate() must return a (points, facets) tuple",
        ));
    }

    let points = tessellation.get_item(0)?;
    let facets = tessellation.get_item(1)?;
    let points: Vec<_> = points
        .try_iter()?
        .map(|item| point(&item?))
        .collect::<PyResult<_>>()?;
    let facets: Vec<[u32; 3]> = facets
        .try_iter()?
        .map(|item| index_triple(&item?))
        .collect::<PyResult<_>>()?;
    Ok((points, facets))
}

/// `MeshPart.wireFromSegment(mesh, group)` → `list[Part.Wire]`.
///
/// `group` is a Python list of 0-based facet indices (as returned by
/// `Mesh.getPlanarSegments`, or by `Mesh.getSegment` after a segmented
/// `meshFromShape`). The boundary loops are computed on the **local** kernel and
/// each one is handed to `Part.makePolygon(points, True)` in upstream's
/// **reverse** boundary order, so a placed mesh's wires are unplaced. An
/// out-of-range facet index raises `ValueError`.
#[pyfunction]
fn wireFromSegment(
    py: Python<'_>,
    mesh: &Bound<'_, PyAny>,
    group: &Bound<'_, PyAny>,
) -> PyResult<Vec<PyObject>> {
    let mesh = mesh
        .extract::<PyRef<'_, PyMesh>>()
        .map_err(|_| PyTypeError::new_err("MeshPart.wireFromSegment expects a Mesh.Mesh"))?;
    let group: Vec<usize> = group
        .extract::<Vec<usize>>()
        .map_err(|_| PyTypeError::new_err("group must be a list of facet indices"))?;

    let count = mesh.core().count_facets();
    if let Some(&index) = group.iter().find(|&&index| index >= count) {
        return Err(PyValueError::new_err(format!(
            "facet index {index} is out of range for a mesh with {count} facets"
        )));
    }

    let loops = mesh.core().boundary_loops(&group);
    let part = py.import("Part")?;
    let mut wires = Vec::with_capacity(loops.len());
    for boundary in loops {
        let mut points = boundary
            .iter()
            .map(|&index| py_point(py, mesh.core().points()[index as usize]))
            .collect::<PyResult<Vec<Py<PyVector>>>>()?;
        // Upstream's `MeshPart.wireFromSegment` builds the polygon in the
        // reverse boundary order.
        points.reverse();
        let wire = part.call_method1("makePolygon", (PyList::new(py, points)?, true))?;
        wires.push(wire.unbind());
    }
    Ok(wires)
}

/// The `MeshPart` module.
#[pymodule]
pub fn MeshPart(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("_kernel_id", ferrocad_core::application_id())?;
    m.add_function(wrap_pyfunction!(meshFromShape, m)?)?;
    m.add_function(wrap_pyfunction!(wireFromSegment, m)?)?;
    Ok(())
}
