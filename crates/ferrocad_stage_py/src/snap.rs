//! Camera-free snapping for Python: the `FreeCADGui.Snapper` counterpart.
//!
//! [`SnapContext`] wraps [`ferrocad_stage::SnapContext`] (a workplane plus its minor and
//! major steps) and resolves a **world-space** point with
//! [`SnapContext::snap_point`](crate::SnapContext::snap_point) — the headless analogue of
//! `snap_to_geometry_with`, which needs a camera. Feature snaps win over the grid; every
//! result is a [`Snap`].

use ferrocad_stage as fs;
use ferrocad_types::Vector3;
use pyo3::prelude::*;
use pyo3::types::PyType;

use crate::workplane::Workplane;

/// What a [`Snap`] landed on (`ferrocad_stage::SnapKind`).
#[pyclass(eq, module = "ferrocad_stage", name = "SnapKind")]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SnapKind {
    /// A point on the picked geometry (or the workplane's projected point).
    Point,
    /// A node of the active workplane's grid.
    Grid,
    /// A topological vertex of the hovered object.
    Vertex,
    /// The midpoint of a topological edge of the hovered object.
    EdgeMid,
    /// The centroid of the hovered planar face.
    Centre,
    /// The active workplane's origin.
    Origin,
}

impl From<SnapKind> for fs::SnapKind {
    fn from(kind: SnapKind) -> Self {
        match kind {
            SnapKind::Point => fs::SnapKind::Point,
            SnapKind::Grid => fs::SnapKind::Grid,
            SnapKind::Vertex => fs::SnapKind::Vertex,
            SnapKind::EdgeMid => fs::SnapKind::EdgeMid,
            SnapKind::Centre => fs::SnapKind::Centre,
            SnapKind::Origin => fs::SnapKind::Origin,
        }
    }
}

impl From<fs::SnapKind> for SnapKind {
    fn from(kind: fs::SnapKind) -> Self {
        match kind {
            fs::SnapKind::Point => SnapKind::Point,
            fs::SnapKind::Grid => SnapKind::Grid,
            fs::SnapKind::Vertex => SnapKind::Vertex,
            fs::SnapKind::EdgeMid => SnapKind::EdgeMid,
            fs::SnapKind::Centre => SnapKind::Centre,
            fs::SnapKind::Origin => SnapKind::Origin,
        }
    }
}

/// A resolved snap (`ferrocad_stage::Snap`).
#[pyclass(module = "ferrocad_stage", name = "Snap")]
#[derive(Clone, Copy)]
pub struct Snap {
    pub(crate) inner: fs::Snap,
}

#[pymethods]
impl Snap {
    /// The snapped world-space point (`(x, y, z)`).
    #[getter]
    fn point(&self) -> (f64, f64, f64) {
        (
            f64::from(self.inner.point[0]),
            f64::from(self.inner.point[1]),
            f64::from(self.inner.point[2]),
        )
    }

    /// What it snapped to.
    #[getter]
    fn kind(&self) -> SnapKind {
        self.inner.kind.into()
    }

    /// The local `(u, v)` on the active workplane, when one is known.
    #[getter]
    fn local(&self) -> Option<(f64, f64)> {
        self.inner.local.map(|local| (local[0], local[1]))
    }

    /// Whether a grid snap landed on a **major** intersection.
    #[getter]
    fn major(&self) -> bool {
        self.inner.major
    }

    fn __repr__(&self) -> String {
        format!(
            "Snap(point={:?}, kind={:?}, local={:?}, major={})",
            self.point(),
            self.inner.kind,
            self.inner.local,
            self.inner.major
        )
    }
}

/// A world-space point a snap may resolve to (`ferrocad_stage::SnapCandidate`).
#[pyclass(module = "ferrocad_stage", name = "SnapCandidate")]
#[derive(Clone, Copy)]
pub struct SnapCandidate {
    pub(crate) inner: fs::SnapCandidate,
}

#[pymethods]
impl SnapCandidate {
    /// A candidate at world `point` (a `(x, y, z)` tuple) of `kind`.
    #[new]
    fn new(point: (f64, f64, f64), kind: SnapKind) -> Self {
        Self {
            inner: fs::SnapCandidate {
                point: vector(point),
                kind: kind.into(),
            },
        }
    }

    /// The candidate's world-space point.
    #[getter]
    fn point(&self) -> (f64, f64, f64) {
        triple(self.inner.point)
    }

    /// The kind of feature this candidate is.
    #[getter]
    fn kind(&self) -> SnapKind {
        self.inner.kind.into()
    }

    fn __repr__(&self) -> String {
        format!(
            "SnapCandidate(point={:?}, kind={:?})",
            triple(self.inner.point),
            self.inner.kind
        )
    }
}

/// The plane and steps a snap is resolved against (`ferrocad_stage::SnapContext`).
#[pyclass(module = "ferrocad_stage", name = "SnapContext")]
#[derive(Clone)]
pub struct SnapContext {
    pub(crate) inner: fs::SnapContext,
}

#[pymethods]
impl SnapContext {
    /// A context over `workplane` with `minor` and `major` steps.
    #[new]
    fn new(workplane: Workplane, minor: f64, major: f64) -> Self {
        Self {
            inner: fs::SnapContext {
                workplane: workplane.inner,
                minor,
                major,
            },
        }
    }

    /// The world `XY` plane with `minor` and a decade-major step.
    #[classmethod]
    fn world(_cls: &Bound<'_, PyType>, minor: f64) -> Self {
        Self {
            inner: fs::SnapContext::world(minor),
        }
    }

    /// The workplane the grid lives on.
    #[getter]
    fn workplane(&self) -> Workplane {
        Workplane {
            inner: self.inner.workplane.clone(),
        }
    }

    /// The minor (finest) grid step.
    #[getter]
    fn minor(&self) -> f64 {
        self.inner.minor
    }

    /// The major step.
    #[getter]
    fn major(&self) -> f64 {
        self.inner.major
    }

    /// Resolve a snap for a world-space `point`.
    ///
    /// A candidate within `tolerance` (Euclidean world distance) wins, ties going to the
    /// earlier candidate. Otherwise `point` is projected onto the workplane and its local
    /// `(u, v)` rounded to `minor`, reporting whether it is a `major` intersection.
    /// `tolerance <= 0` disables feature snaps; `minor <= 0` returns the projected point
    /// as `SnapKind.Point`.
    #[pyo3(signature = (point, candidates=Vec::new(), tolerance=0.0), text_signature = "(self, point, candidates=(), tolerance=0.0)")]
    fn snap_point(
        &self,
        point: (f64, f64, f64),
        candidates: Vec<SnapCandidate>,
        tolerance: f64,
    ) -> Snap {
        let candidates: Vec<fs::SnapCandidate> =
            candidates.into_iter().map(|candidate| candidate.inner).collect();
        Snap {
            inner: self.inner.snap_point(vector(point), &candidates, tolerance),
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "SnapContext(workplane={:?}, minor={}, major={})",
            self.inner.workplane, self.inner.minor, self.inner.major
        )
    }
}

/// Register this module's classes.
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<SnapKind>()?;
    m.add_class::<Snap>()?;
    m.add_class::<SnapCandidate>()?;
    m.add_class::<SnapContext>()?;
    Ok(())
}

fn vector((x, y, z): (f64, f64, f64)) -> Vector3 {
    Vector3::new(x, y, z)
}

fn triple(v: Vector3) -> (f64, f64, f64) {
    (v.x, v.y, v.z)
}
