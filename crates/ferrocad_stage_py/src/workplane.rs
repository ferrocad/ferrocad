//! Workplanes for Python: the planar frame a grid and a sketch live on.
//!
//! [`Workplane`] wraps [`ferrocad_stage::Workplane`] — an origin plus two in-plane
//! axes, normalized on construction, with `n = u × v` — so a headless script can map
//! between local `(u, v)` and world coordinates. [`WorkplaneKind`] says where a frame
//! came from (the world `XY` plane, a named datum, or a planar face); the Python
//! constructor takes it as an optional argument so the native kinds stay reachable.

use ferrocad_stage as fs;
use ferrocad_types::Vector3;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyType;

/// Where a [`Workplane`] came from (`ferrocad_stage::WorkplaneKind`).
///
/// `WorkplaneKind.World` is the world `XY` plane; `WorkplaneKind.Datum(name)` and
/// `WorkplaneKind.Face(entity, element)` name a document datum or a planar face. The
/// readable form is the `kind` property: `"World"`, `("Datum", name)` or
/// `("Face", entity, element)`.
#[pyclass(eq, module = "ferrocad_stage", name = "WorkplaneKind")]
#[derive(Clone, PartialEq, Eq)]
pub struct WorkplaneKind {
    pub(crate) inner: fs::WorkplaneKind,
}

#[pymethods]
impl WorkplaneKind {
    /// The world `XY` plane's kind.
    #[classattr]
    #[pyo3(name = "World")]
    fn world() -> Self {
        Self {
            inner: fs::WorkplaneKind::World,
        }
    }

    /// A named datum plane.
    #[staticmethod]
    #[pyo3(name = "Datum")]
    fn datum(name: String) -> Self {
        Self {
            inner: fs::WorkplaneKind::Datum(name),
        }
    }

    /// A planar face of the object `entity`, the sub-element `element` (e.g. `"Face6"`).
    #[staticmethod]
    #[pyo3(name = "Face")]
    fn face(entity: u32, element: String) -> Self {
        Self {
            inner: fs::WorkplaneKind::Face { entity, element },
        }
    }

    /// The readable value: `"World"`, `("Datum", name)` or `("Face", entity, element)`.
    #[getter]
    fn kind(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        use pyo3::IntoPyObjectExt;
        match &self.inner {
            fs::WorkplaneKind::World => "World".into_py_any(py),
            fs::WorkplaneKind::Datum(name) => ("Datum", name.as_str()).into_py_any(py),
            fs::WorkplaneKind::Face { entity, element } => {
                ("Face", *entity, element.as_str()).into_py_any(py)
            }
        }
    }

    fn __repr__(&self) -> String {
        match &self.inner {
            fs::WorkplaneKind::World => "WorkplaneKind.World".to_string(),
            fs::WorkplaneKind::Datum(name) => format!("WorkplaneKind.Datum({name:?})"),
            fs::WorkplaneKind::Face { entity, element } => {
                format!("WorkplaneKind.Face({entity}, {element:?})")
            }
        }
    }
}

impl From<fs::WorkplaneKind> for WorkplaneKind {
    fn from(kind: fs::WorkplaneKind) -> Self {
        Self { inner: kind }
    }
}

/// A right-handed planar frame (`ferrocad_stage::Workplane`).
///
/// `n = u × v`; the axes are normalized and must be non-degenerate and not parallel,
/// otherwise the constructor raises `ValueError`.
#[pyclass(module = "ferrocad_stage", name = "Workplane")]
#[derive(Clone)]
pub struct Workplane {
    pub(crate) inner: fs::Workplane,
}

#[pymethods]
impl Workplane {
    /// A frame from an origin and two in-plane axes (world kind by default).
    #[new]
    #[pyo3(signature = (origin=(0.0, 0.0, 0.0), u=(1.0, 0.0, 0.0), v=(0.0, 1.0, 0.0), kind=None), text_signature = "(origin=(0, 0, 0), u=(1, 0, 0), v=(0, 1, 0), kind=None)")]
    fn new(
        origin: (f64, f64, f64),
        u: (f64, f64, f64),
        v: (f64, f64, f64),
        kind: Option<WorkplaneKind>,
    ) -> PyResult<Self> {
        let kind = kind.map_or(fs::WorkplaneKind::World, |kind| kind.inner);
        fs::Workplane::new(vector(origin), vector(u), vector(v), kind)
            .map(|inner| Self { inner })
            .ok_or_else(|| {
                PyValueError::new_err(
                    "the workplane axes must be non-degenerate and not parallel",
                )
            })
    }

    /// The world `XY` plane: `origin = 0`, `u = +X`, `v = +Y`, `n = +Z`.
    #[classmethod]
    fn world(_cls: &Bound<'_, PyType>) -> Self {
        Self {
            inner: fs::Workplane::world(),
        }
    }

    /// The origin.
    #[getter]
    fn origin(&self) -> (f64, f64, f64) {
        triple(self.inner.origin)
    }

    /// The `u` axis (normalized).
    #[getter]
    fn u(&self) -> (f64, f64, f64) {
        triple(self.inner.u)
    }

    /// The `v` axis (normalized).
    #[getter]
    fn v(&self) -> (f64, f64, f64) {
        triple(self.inner.v)
    }

    /// The normal, `u × v`, normalized.
    #[getter]
    fn n(&self) -> (f64, f64, f64) {
        triple(self.inner.n)
    }

    /// Where this frame came from.
    #[getter]
    fn kind(&self) -> WorkplaneKind {
        self.inner.kind.clone().into()
    }

    /// The world point at local `(a, b)`.
    fn point(&self, a: f64, b: f64) -> (f64, f64, f64) {
        triple(self.inner.point(a, b))
    }

    /// The local `(u, v)` of a world point (its normal component is dropped).
    fn project(&self, point: (f64, f64, f64)) -> (f64, f64) {
        let local = self.inner.project(vector(point));
        (local[0], local[1])
    }

    fn __repr__(&self) -> String {
        format!(
            "Workplane(origin={:?}, u={:?}, v={:?}, kind={:?})",
            triple(self.inner.origin),
            triple(self.inner.u),
            triple(self.inner.v),
            self.inner.kind
        )
    }
}

/// Register this module's classes.
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<WorkplaneKind>()?;
    m.add_class::<Workplane>()?;
    Ok(())
}

fn vector((x, y, z): (f64, f64, f64)) -> Vector3 {
    Vector3::new(x, y, z)
}

fn triple(v: Vector3) -> (f64, f64, f64) {
    (v.x, v.y, v.z)
}
