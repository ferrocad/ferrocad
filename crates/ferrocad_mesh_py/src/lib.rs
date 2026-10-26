//! # FerroCAD Mesh Python modules (`ferrocad_mesh_py`)
//!
//! Two built-in modules over [`ferrocad_mesh`]:
//!
//! - **`Mesh`** — `Mesh.Mesh`, `Mesh.Facet` and `Mesh.createBox`, plus the
//!   property conversion that makes `obj.Mesh` a `Mesh.Mesh` for a
//!   `Mesh::Feature`. The mesh placement is stored beside the points and is
//!   never baked: `Topology`/`BoundBox` are placed, `Points`/`Facet.Points` are
//!   local, and the STL writer emits the local kernel — upstream's split.
//! - **`MeshPart`** — `meshFromShape` (over a duck-typed `Shape.tessellate`) and
//!   `wireFromSegment` (boundary loops from the local kernel, built into
//!   `Part.Wire`s through Python). Nothing here imports `Part` in Rust.
//!
//! Both modules must live in the same image as `ferrocad` (and `Part`, for
//! `wireFromSegment`) so they share one core registry; see
//! `docs/tasks/mesh-module.md` §4.3. An embedding application registers the
//! modules with [`pyo3::append_to_inittab!`] before the interpreter starts, or
//! calls [`install`] on a running interpreter.

#![allow(non_snake_case)]

use std::path::PathBuf;

use ferrocad_core::{ExtensionData, ExtensionValue};
use ferrocad_mesh::{Mesh as CoreMesh, MeshError, MeshProperty};
use ferrocad_py::{PyBoundBox, PyPlacement, PyVector};
use ferrocad_types::Vector3;
use pyo3::exceptions::{PyIOError, PyNotImplementedError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyModule, PyTuple};

mod meshpart;

pub use meshpart::MeshPart;

// ---------------------------------------------------------------------------
// Shared conversions
// ---------------------------------------------------------------------------

/// Convert a mesh-level error to the matching Python exception.
fn mesh_error(error: MeshError) -> PyErr {
    match error {
        MeshError::Io(error) => PyIOError::new_err(error.to_string()),
        other => PyValueError::new_err(other.to_string()),
    }
}

/// Read a point: an `App.Vector` (anything with numeric `.x`/`.y`/`.z`) or a
/// 3-float sequence.
///
/// Upstream's tuple constructor silently skipped non-`Base.Vector` points and
/// crashed later; this helper is deliberately strict and raises `TypeError`
/// instead (see `simplecad/mesh/stretch.py:77-79`).
pub(crate) fn point(value: &Bound<'_, PyAny>) -> PyResult<Vector3> {
    if let (Ok(x), Ok(y), Ok(z)) = (
        value.getattr("x").and_then(|v| v.extract::<f64>()),
        value.getattr("y").and_then(|v| v.extract::<f64>()),
        value.getattr("z").and_then(|v| v.extract::<f64>()),
    ) {
        return Ok(Vector3::new(x, y, z));
    }
    if let Ok(sequence) = value.extract::<Vec<f64>>() {
        if sequence.len() == 3 {
            return Ok(Vector3::new(sequence[0], sequence[1], sequence[2]));
        }
    }
    Err(PyTypeError::new_err(format!(
        "expected an App.Vector or a 3-float sequence, got {}",
        value
            .get_type()
            .name()
            .map(|name| name.to_string())
            .unwrap_or_else(|_| "an unknown type".to_string())
    )))
}

/// Wrap a local point as an `App.Vector`.
fn py_point(py: Python<'_>, point: Vector3) -> PyResult<Py<PyVector>> {
    Py::new(py, PyVector::fresh(point))
}

/// Read a facet as three point indices.
fn index_triple(value: &Bound<'_, PyAny>) -> PyResult<[u32; 3]> {
    let items: Vec<Bound<'_, PyAny>> = value.try_iter()?.collect::<PyResult<_>>()?;
    if items.len() != 3 {
        return Err(PyTypeError::new_err(
            "a facet must be a sequence of three point indices",
        ));
    }
    let mut facet = [0u32; 3];
    for (slot, item) in facet.iter_mut().zip(&items) {
        *slot = item
            .extract::<u32>()
            .map_err(|_| PyTypeError::new_err("a facet index must be a non-negative integer"))?;
    }
    Ok(facet)
}

/// Whether `value` is a 3-index sequence (used to tell the `(points, facets)`
/// tuple constructor from a list of triangles).
fn is_index_triple(value: &Bound<'_, PyAny>) -> bool {
    let Ok(items) = value.try_iter() else {
        return false;
    };
    let mut count = 0;
    for item in items {
        let Ok(item) = item else {
            return false;
        };
        if item.extract::<u32>().is_err() {
            return false;
        }
        count += 1;
        if count > 3 {
            return false;
        }
    }
    count == 3
}

/// Whether `value` looks like the point half of the `(points, facets)` form.
fn is_point_sequence(value: &Bound<'_, PyAny>) -> bool {
    match value.try_iter() {
        Ok(mut items) => match items.next() {
            Some(Ok(first)) => point(&first).is_ok(),
            _ => false,
        },
        Err(_) => false,
    }
}

/// Whether a single constructor argument is the `(points, facets)` form.
///
/// A tuple of two is the upstream shape. A two-item list is also accepted when
/// its second item is a sequence of index triples — this cannot collide with a
/// list of two triangles, whose second triangle is a sequence of points, not of
/// integers.
fn is_points_and_facets(value: &Bound<'_, PyAny>) -> bool {
    if value.downcast::<PyTuple>().is_ok() {
        return value.len().is_ok_and(|len| len == 2);
    }
    let Ok(len) = value.len() else {
        return false;
    };
    if len != 2 {
        return false;
    }
    let (Ok(points), Ok(facets)) = (value.get_item(0), value.get_item(1)) else {
        return false;
    };
    if !is_point_sequence(&points) {
        return false;
    }
    match facets.try_iter() {
        Ok(mut items) => match items.next() {
            Some(Ok(first)) => is_index_triple(&first),
            _ => false,
        },
        Err(_) => false,
    }
}

/// Read the `(points, facets)` constructor form.
fn points_and_facets(points: &Bound<'_, PyAny>, facets: &Bound<'_, PyAny>) -> PyResult<CoreMesh> {
    let points = points
        .try_iter()?
        .map(|item| point(&item?))
        .collect::<PyResult<Vec<_>>>()?;
    let facets = facets
        .try_iter()?
        .map(|item| index_triple(&item?))
        .collect::<PyResult<Vec<_>>>()?;
    CoreMesh::new(points, facets).map_err(mesh_error)
}

/// The unit normal of a local triangle, or zero when it is degenerate.
fn facet_normal(points: [Vector3; 3]) -> Vector3 {
    let normal = points[1].sub(&points[0]).cross(&points[2].sub(&points[0]));
    let length = normal.length();
    if length > 0.0 && length.is_finite() {
        normal.scale(1.0 / length)
    } else {
        Vector3::zero()
    }
}

// ---------------------------------------------------------------------------
// Facet
// ---------------------------------------------------------------------------

/// `Mesh.Facet`: three **local** points and their unit normal.
#[pyclass(name = "Facet", module = "Mesh")]
#[derive(Clone)]
pub struct PyFacet {
    points: [Vector3; 3],
    normal: Vector3,
}

impl PyFacet {
    fn new(core: &CoreMesh, facet: [u32; 3]) -> Self {
        let points = [
            core.points()[facet[0] as usize],
            core.points()[facet[1] as usize],
            core.points()[facet[2] as usize],
        ];
        Self {
            points,
            normal: facet_normal(points),
        }
    }
}

#[pymethods]
impl PyFacet {
    /// The three facet corner points, in the mesh's local frame.
    #[getter]
    fn Points(&self) -> Vec<PyVector> {
        self.points.iter().copied().map(PyVector::fresh).collect()
    }

    /// The unit facet normal in the mesh's local frame (zero for a degenerate
    /// facet).
    #[getter]
    fn Normal(&self) -> PyVector {
        PyVector::fresh(self.normal)
    }

    fn __repr__(&self) -> String {
        format!(
            "Mesh.Facet({:?}, {:?}, {:?})",
            self.points[0], self.points[1], self.points[2]
        )
    }
}

// ---------------------------------------------------------------------------
// Mesh.Mesh
// ---------------------------------------------------------------------------

/// `Mesh.Mesh`: a triangle mesh with a live placement.
#[pyclass(name = "Mesh", module = "Mesh")]
#[derive(Clone)]
pub struct PyMesh {
    inner: CoreMesh,
}

impl PyMesh {
    pub(crate) fn from_core(inner: CoreMesh) -> Self {
        Self { inner }
    }

    /// The stored mesh (with its placement).
    pub(crate) fn core(&self) -> &CoreMesh {
        &self.inner
    }

    fn from_single(value: &Bound<'_, PyAny>) -> PyResult<Self> {
        if let Ok(other) = value.extract::<PyRef<'_, PyMesh>>() {
            return Ok(other.clone());
        }
        if let Ok(path) = value.extract::<PathBuf>() {
            return Self::read(&path);
        }
        if is_points_and_facets(value) {
            let points = value.get_item(0)?;
            let facets = value.get_item(1)?;
            return Ok(Self::from_core(points_and_facets(&points, &facets)?));
        }
        Self::from_triangles(value)
    }

    /// `Mesh.Mesh([tri, tri, …])`: each `tri` is a `Facet`, a sequence of three
    /// points, or nine floats; a flat list of 3-float points is also accepted
    /// (three consecutive points per facet). Shared corners are welded.
    fn from_triangles(value: &Bound<'_, PyAny>) -> PyResult<Self> {
        let mut triangles: Vec<[Vector3; 3]> = Vec::new();
        let mut pending: Vec<Vector3> = Vec::new();

        for item in value.try_iter()? {
            let item = item?;
            if let Ok(facet) = item.extract::<PyRef<'_, PyFacet>>() {
                triangles.push(facet.points);
                continue;
            }
            let len = item.len().map_err(|_| {
                PyTypeError::new_err(
                    "a mesh constructor list must hold triangles (3 points or 9 floats) \
                     or points (3 floats)",
                )
            })?;
            if len == 9 {
                let coordinates = item.extract::<Vec<f64>>().map_err(|_| {
                    PyTypeError::new_err("a nine-value triangle must be nine numbers")
                })?;
                triangles.push([
                    Vector3::new(coordinates[0], coordinates[1], coordinates[2]),
                    Vector3::new(coordinates[3], coordinates[4], coordinates[5]),
                    Vector3::new(coordinates[6], coordinates[7], coordinates[8]),
                ]);
                continue;
            }
            if len == 3 {
                if let Ok(point) = point(&item) {
                    pending.push(point);
                    if pending.len() == 3 {
                        triangles.push([pending[0], pending[1], pending[2]]);
                        pending.clear();
                    }
                    continue;
                }
                let corners: Vec<Vector3> = item
                    .try_iter()?
                    .map(|corner| point(&corner?))
                    .collect::<PyResult<_>>()?;
                if corners.len() != 3 {
                    return Err(PyTypeError::new_err(
                        "a triangle must have exactly three points",
                    ));
                }
                triangles.push([corners[0], corners[1], corners[2]]);
                continue;
            }
            return Err(PyTypeError::new_err(
                "a mesh constructor list must hold triangles (3 points or 9 floats) \
                 or points (3 floats)",
            ));
        }

        if !pending.is_empty() {
            return Err(PyValueError::new_err(
                "a flat point list must hold a multiple of three points",
            ));
        }
        CoreMesh::from_facet_points(&triangles)
            .map(Self::from_core)
            .map_err(mesh_error)
    }

    fn read(path: &std::path::Path) -> PyResult<Self> {
        check_suffix(path)?;
        CoreMesh::read_stl(path)
            .map(Self::from_core)
            .map_err(mesh_error)
    }
}

/// The one file suffix this slice supports (`Mesh.Mesh(path)` / `write(path)`).
const SUPPORTED_SUFFIX: &str = "stl";

/// Reject a path whose suffix is not the supported one, naming it.
fn check_suffix(path: &std::path::Path) -> PyResult<()> {
    let suffix = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("");
    if suffix.eq_ignore_ascii_case(SUPPORTED_SUFFIX) {
        return Ok(());
    }
    let found = if suffix.is_empty() {
        "no suffix".to_string()
    } else {
        format!(".{suffix}")
    };
    Err(PyValueError::new_err(format!(
        "unsupported mesh file format ({found}): only .{SUPPORTED_SUFFIX} is supported"
    )))
}

#[pymethods]
impl PyMesh {
    /// `Mesh.Mesh()` — empty; `Mesh.Mesh(other)` — copy (placement included);
    /// `Mesh.Mesh((points, facets))`; `Mesh.Mesh([tri, …])`; `Mesh.Mesh(path)`.
    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<Self> {
        match args.len() {
            0 => Ok(Self::from_core(CoreMesh::empty())),
            1 => Self::from_single(&args.get_item(0)?),
            _ => Err(PyTypeError::new_err(
                "Mesh.Mesh() takes at most one argument",
            )),
        }
    }

    /// The stored points with the mesh placement applied, and the stored facets.
    ///
    /// This is the fingerprinting form (`simplecad/mesh/conversion.py`): the
    /// point order is preserved, so two reads compare equal.
    #[getter]
    fn Topology(&self, py: Python<'_>) -> PyResult<(Vec<Py<PyVector>>, Vec<(u32, u32, u32)>)> {
        let points = self
            .inner
            .placed_points()
            .into_iter()
            .map(|point| py_point(py, point))
            .collect::<PyResult<_>>()?;
        let facets = self
            .inner
            .facets()
            .iter()
            .map(|facet| (facet[0], facet[1], facet[2]))
            .collect();
        Ok((points, facets))
    }

    /// The **local** points (the placement is not applied).
    #[getter]
    fn Points(&self, py: Python<'_>) -> PyResult<Vec<Py<PyVector>>> {
        self.inner
            .points()
            .iter()
            .copied()
            .map(|point| py_point(py, point))
            .collect()
    }

    /// The facets as `Mesh.Facet` values, in the mesh's local frame.
    #[getter]
    fn Facets(&self, py: Python<'_>) -> PyResult<Vec<Py<PyFacet>>> {
        self.inner
            .facets()
            .iter()
            .map(|facet| Py::new(py, PyFacet::new(&self.inner, *facet)))
            .collect()
    }

    #[getter]
    fn CountPoints(&self) -> usize {
        self.inner.count_points()
    }

    #[getter]
    fn CountFacets(&self) -> usize {
        self.inner.count_facets()
    }

    /// The **placed** axis-aligned bounding box.
    #[getter]
    fn BoundBox(&self) -> PyBoundBox {
        let bounds = self.inner.placed_bounds();
        PyBoundBox::fresh(bounds.min, bounds.max)
    }

    /// The absolute volume (`fabs` of the tetrahedron sum, upstream).
    #[getter]
    fn Volume(&self) -> f64 {
        self.inner.volume()
    }

    /// The sum of the local facet areas.
    #[getter]
    fn Area(&self) -> f64 {
        self.inner.area()
    }

    /// The stored placement; setting it is absolute and never bakes the points.
    #[getter]
    fn Placement(&self) -> PyPlacement {
        PyPlacement::fresh(self.inner.placement())
    }

    #[setter]
    fn set_Placement(&mut self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let placement = value
            .extract::<PyRef<'_, PyPlacement>>()
            .map_err(|_| PyTypeError::new_err("Mesh.Placement must be an App.Placement"))?;
        self.inner.set_placement(placement.get());
        Ok(())
    }

    /// No border edges (upstream `isSolid`); manifoldness and orientation are
    /// reported separately.
    fn isSolid(&self) -> bool {
        self.inner.is_solid()
    }

    /// Whether an edge is shared by more than two facets.
    fn hasNonManifolds(&self) -> bool {
        self.inner.has_non_manifolds()
    }

    /// Whether two facets traverse a shared edge in the same direction.
    fn hasNonUniformOrientedFacets(&self) -> bool {
        self.inner.has_non_uniform_oriented_facets()
    }

    fn hasSelfIntersections(&self) -> bool {
        self.inner.has_self_intersections()
    }

    /// `[(facet_a, facet_b, point, point), …]` with local points.
    fn getSelfIntersections(
        &self,
        py: Python<'_>,
    ) -> PyResult<Vec<(usize, usize, Py<PyVector>, Py<PyVector>)>> {
        self.inner
            .self_intersections()
            .into_iter()
            .map(|intersection| {
                Ok((
                    intersection.first,
                    intersection.second,
                    py_point(py, intersection.points[0])?,
                    py_point(py, intersection.points[1])?,
                ))
            })
            .collect()
    }

    /// A deep copy, placement included.
    fn copy(&self) -> Self {
        self.clone()
    }

    /// Write binary STL from the **local** points.
    fn write(&self, path: PathBuf) -> PyResult<()> {
        check_suffix(&path)?;
        self.inner
            .write_stl(&path)
            .map_err(|error| PyIOError::new_err(error.to_string()))
    }

    fn removeDuplicatedPoints(&mut self) {
        self.inner.remove_duplicated_points();
    }

    fn removeDuplicatedFacets(&mut self) {
        self.inner.remove_duplicated_facets();
    }

    fn fixDegenerations(&mut self) {
        self.inner.fix_degenerations();
    }

    fn harmonizeNormals(&mut self) {
        self.inner.harmonize_normals();
    }

    /// One mesh per connected component; each keeps this mesh's placement.
    fn getSeparateComponents(&self, py: Python<'_>) -> PyResult<Vec<Py<PyMesh>>> {
        self.inner
            .separate_components()
            .into_iter()
            .map(|component| Py::new(py, Self::from_core(component)))
            .collect()
    }

    /// Groups of adjacent coplanar facets, within a **distance** deviation.
    #[pyo3(signature = (deviation, minFacets = 0))]
    fn getPlanarSegments(&self, deviation: f64, minFacets: usize) -> Vec<Vec<usize>> {
        self.inner.planar_segments(deviation, minFacets)
    }

    fn countComponents(&self) -> usize {
        self.inner.count_components()
    }

    fn __repr__(&self) -> String {
        format!(
            "Mesh.Mesh({} points, {} facets)",
            self.inner.count_points(),
            self.inner.count_facets()
        )
    }

    fn __len__(&self) -> usize {
        self.inner.count_facets()
    }
}

// ---------------------------------------------------------------------------
// Mesh module functions
// ---------------------------------------------------------------------------

/// `Mesh.createBox(length=10, width=10, height=10, edgelen=0)`.
///
/// The box is centred on the origin (`-l/2 … +l/2`), as upstream's
/// `BuildRegularGeoms.Cube` is. The fourth upstream argument (`edgelen`, edge
/// subdivision) is accepted but not implemented; a nonzero value raises
/// `NotImplementedError` rather than silently returning a different resolution.
#[pyfunction]
#[pyo3(signature = (length = 10.0, width = 10.0, height = 10.0, edgelen = 0.0))]
fn createBox(length: f64, width: f64, height: f64, edgelen: f64) -> PyResult<PyMesh> {
    if edgelen != 0.0 {
        return Err(PyNotImplementedError::new_err(
            "Mesh.createBox(..., edgelen) is not implemented; only edgelen=0 is supported",
        ));
    }
    CoreMesh::box_(length, width, height)
        .map(PyMesh::from_core)
        .map_err(mesh_error)
}

// ---------------------------------------------------------------------------
// Mesh::PropertyMeshKernel ↔ Mesh.Mesh
// ---------------------------------------------------------------------------

/// `Mesh::PropertyMeshKernel` → `Mesh.Mesh` (a snapshot in the object's frame).
fn mesh_to_py(py: Python<'_>, data: &dyn ExtensionData) -> Option<PyObject> {
    let property = data.as_any().downcast_ref::<MeshProperty>()?;
    Py::new(py, PyMesh::from_core(property.mesh().clone()))
        .ok()
        .map(Py::into_any)
}

/// `Mesh.Mesh` → `Mesh::PropertyMeshKernel` (stores the local points and the
/// placement, which `Mesh::Feature` then adopts).
fn mesh_to_extension(value: &Bound<'_, PyAny>) -> Option<ExtensionValue> {
    let mesh = value.extract::<PyRef<'_, PyMesh>>().ok()?;
    Some(ExtensionValue::new(Box::new(MeshProperty::new(
        mesh.inner.clone(),
    ))))
}

// ---------------------------------------------------------------------------
// Modules
// ---------------------------------------------------------------------------

/// The `Mesh` module.
#[pymodule]
pub fn Mesh(m: &Bound<'_, PyModule>) -> PyResult<()> {
    ferrocad_mesh::register();
    ferrocad_py::register_extension_converter(
        ferrocad_mesh::TYPE_NAME,
        mesh_to_py,
        mesh_to_extension,
    );
    m.add_class::<PyMesh>()?;
    m.add_class::<PyFacet>()?;
    m.add("_kernel_id", ferrocad_core::application_id())?;
    m.add_function(wrap_pyfunction!(createBox, m)?)?;
    Ok(())
}

/// Insert `Mesh` and `MeshPart` into a running interpreter's `sys.modules`,
/// returning the `Mesh` module.
///
/// This is the runtime counterpart of [`pyo3::append_to_inittab!`], for a host
/// that boots the interpreter before it links this crate. Idempotent: an
/// already-imported module is reused (so its registry state is not replaced).
pub fn install(py: Python<'_>) -> PyResult<Bound<'_, PyModule>> {
    let modules: Bound<'_, PyDict> = py.import("sys")?.getattr("modules")?.downcast_into()?;

    let mesh = match modules.get_item("Mesh")? {
        Some(existing) => existing.downcast_into::<PyModule>()?,
        None => {
            let mesh = pyo3::wrap_pymodule!(Mesh)(py).into_bound(py);
            modules.set_item("Mesh", &mesh)?;
            mesh
        }
    };

    if modules.get_item("MeshPart")?.is_none() {
        let mesh_part = pyo3::wrap_pymodule!(MeshPart)(py).into_bound(py);
        modules.set_item("MeshPart", &mesh_part)?;
    }

    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_box_is_origin_centred_and_solid() {
        let mesh = createBox(10.0, 20.0, 30.0, 0.0).expect("createBox");
        assert_eq!(mesh.inner.count_points(), 8);
        assert_eq!(mesh.inner.count_facets(), 12);
        assert!(mesh.inner.is_solid());
        assert!(!mesh.inner.has_non_manifolds());
        assert!((mesh.inner.volume() - 6000.0).abs() < 1e-9);

        let bounds = mesh.inner.bounds();
        assert_eq!(bounds.min, Vector3::new(-5.0, -10.0, -15.0));
        assert_eq!(bounds.max, Vector3::new(5.0, 10.0, 15.0));
    }

    #[test]
    fn a_nonzero_edgelen_is_not_implemented() {
        let error = createBox(1.0, 1.0, 1.0, 2.0)
            .err()
            .expect("edgelen is deferred");
        Python::with_gil(|py| {
            assert!(error.is_instance_of::<PyNotImplementedError>(py));
        });
    }

    #[test]
    fn a_facet_exposes_local_points_and_a_unit_normal() {
        let mesh = CoreMesh::box_(2.0, 2.0, 2.0).expect("box");
        let facet = PyFacet::new(&mesh, mesh.facets()[0]);
        assert_eq!(
            facet.points,
            [
                mesh.points()[mesh.facets()[0][0] as usize],
                mesh.points()[mesh.facets()[0][1] as usize],
                mesh.points()[mesh.facets()[0][2] as usize],
            ]
        );
        assert!((facet.normal.length() - 1.0).abs() < 1e-9);
        assert!(
            facet.normal.z.abs() > 0.99,
            "the first box facet is the bottom"
        );
    }

    #[test]
    fn install_registers_both_modules_in_a_running_interpreter() {
        Python::with_gil(|py| {
            let mesh = install(py).expect("install");
            assert_eq!(mesh.name().expect("module name").to_string(), "Mesh");
            let modules = py
                .import("sys")
                .unwrap()
                .getattr("modules")
                .unwrap()
                .downcast_into::<PyDict>()
                .unwrap();
            assert!(modules.get_item("Mesh").unwrap().is_some());
            assert!(modules.get_item("MeshPart").unwrap().is_some());
        });
    }

    #[test]
    fn the_property_converter_round_trips_a_mesh_and_its_placement() {
        let mut mesh = CoreMesh::box_(1.0, 2.0, 3.0).expect("box");
        let placement = ferrocad_types::Placement::new(
            Vector3::new(4.0, 5.0, 6.0),
            ferrocad_types::Rotation::identity(),
        );
        mesh.set_placement(placement);

        Python::with_gil(|py| {
            let value = Py::new(py, PyMesh::from_core(mesh.clone())).expect("wrap");
            let extension =
                mesh_to_extension(value.bind(py).as_any()).expect("a Mesh.Mesh converts");
            let property = extension
                .data()
                .as_any()
                .downcast_ref::<MeshProperty>()
                .expect("a MeshProperty");
            assert_eq!(property.mesh(), &mesh);
            assert_eq!(property.mesh().placement(), placement);

            let back = mesh_to_py(py, property).expect("a Mesh.Mesh comes back");
            let back = back
                .bind(py)
                .extract::<PyRef<'_, PyMesh>>()
                .expect("Mesh.Mesh");
            assert_eq!(back.core(), &mesh);
        });
    }
}
