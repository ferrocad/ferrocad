//! The planar fillet and chamfer algorithms behind `Part.ChFi2d`.
//!
//! The OCCT bindings live in `ferrocad_part_cxx::chfi2d`; this module owns the
//! FerroCAD semantics: shape downcasting, `Option` mapping, and the distinction
//! between "no result" and "OCCT raised" that `Part.OCCError` reports.

use cxx::UniquePtr;
use ferrocad_geom::Shape;
use ferrocad_part_cxx::chfi2d;

use crate::shape::{downcast, erase};

macro_rules! with_occt {
    ($shape:expr, |$inner:ident| $body:block) => {{
        if let Ok(occt) = downcast($shape) {
            let guard = occt.borrow();
            if let Some($inner) = guard.as_ref() {
                $body
            }
        }
    }};
}

/// The three edges an algorithm returns: the new fillet/chamfer, then the two
/// trimmed input edges.
pub struct EdgeTriple {
    inner: UniquePtr<chfi2d::ChFi2dEdgeTriple>,
}

impl EdgeTriple {
    fn from_inner(inner: UniquePtr<chfi2d::ChFi2dEdgeTriple>) -> Option<Self> {
        if inner.is_null() {
            None
        } else {
            Some(Self { inner })
        }
    }

    /// The newly built fillet or chamfer edge.
    pub fn edge(&self) -> Option<Shape> {
        if self.inner.first_is_null() {
            None
        } else {
            Some(erase(self.inner.first_shape()))
        }
    }

    /// The first input edge, trimmed to its tangent/chamfer point.
    pub fn first(&self) -> Option<Shape> {
        if self.inner.second_is_null() {
            None
        } else {
            Some(erase(self.inner.second_shape()))
        }
    }

    /// The second input edge, trimmed to its tangent/chamfer point.
    pub fn second(&self) -> Option<Shape> {
        if self.inner.third_is_null() {
            None
        } else {
            Some(erase(self.inner.third_shape()))
        }
    }
}

/// `Part.ChFi2d.FilletAlgo` — the iterative `ChFi2d_FilletAlgo`.
pub struct FilletAlgo {
    inner: UniquePtr<chfi2d::ChFi2dFilletAlgo>,
}

impl FilletAlgo {
    pub fn new() -> Self {
        FilletAlgo {
            inner: chfi2d::chfi2d_fillet_algo_new(),
        }
    }

    pub fn init_wire(&self, wire: &Shape, origin: [f64; 3], normal: [f64; 3]) {
        with_occt!(wire, |wire| {
            let _ = self.inner.init_wire(
                wire,
                origin[0],
                origin[1],
                origin[2],
                normal[0],
                normal[1],
                normal[2],
            );
        });
    }

    pub fn init_edges(
        &self,
        edge1: &Shape,
        edge2: &Shape,
        origin: [f64; 3],
        normal: [f64; 3],
    ) {
        let (Ok(edge1), Ok(edge2)) = (downcast(edge1), downcast(edge2)) else {
            return;
        };
        let guard1 = edge1.borrow();
        let guard2 = edge2.borrow();
        let (Some(edge1), Some(edge2)) = (guard1.as_ref(), guard2.as_ref()) else {
            return;
        };
        let _ = self.inner.init_edges(
            edge1,
            edge2,
            origin[0],
            origin[1],
            origin[2],
            normal[0],
            normal[1],
            normal[2],
        );
    }

    /// `Some(true)` when a fillet was found, `Some(false)` when none was, and
    /// `None` when OCCT raised (for example on an uninitialised algorithm).
    pub fn perform(&self, radius: f64) -> Option<bool> {
        self.inner.perform(radius).ok()
    }

    pub fn number_of_results(&self, point: [f64; 3]) -> Option<i32> {
        self.inner.nb_results(point[0], point[1], point[2]).ok()
    }

    pub fn result(&self, point: [f64; 3], solution: i32) -> Option<EdgeTriple> {
        self.inner
            .result(point[0], point[1], point[2], solution)
            .ok()
            .and_then(EdgeTriple::from_inner)
    }
}

impl Default for FilletAlgo {
    fn default() -> Self {
        Self::new()
    }
}

/// `Part.ChFi2d.AnaFilletAlgo` — the analytic (segment/arc) fillet.
pub struct AnaFilletAlgo {
    inner: UniquePtr<chfi2d::ChFi2dAnaFilletAlgo>,
}

impl AnaFilletAlgo {
    pub fn new() -> Self {
        AnaFilletAlgo {
            inner: chfi2d::chfi2d_ana_fillet_algo_new(),
        }
    }

    pub fn init_wire(&self, wire: &Shape, origin: [f64; 3], normal: [f64; 3]) {
        with_occt!(wire, |wire| {
            let _ = self.inner.init_wire(
                wire,
                origin[0],
                origin[1],
                origin[2],
                normal[0],
                normal[1],
                normal[2],
            );
        });
    }

    pub fn init_edges(
        &self,
        edge1: &Shape,
        edge2: &Shape,
        origin: [f64; 3],
        normal: [f64; 3],
    ) {
        let (Ok(edge1), Ok(edge2)) = (downcast(edge1), downcast(edge2)) else {
            return;
        };
        let guard1 = edge1.borrow();
        let guard2 = edge2.borrow();
        let (Some(edge1), Some(edge2)) = (guard1.as_ref(), guard2.as_ref()) else {
            return;
        };
        let _ = self.inner.init_edges(
            edge1,
            edge2,
            origin[0],
            origin[1],
            origin[2],
            normal[0],
            normal[1],
            normal[2],
        );
    }

    pub fn perform(&self, radius: f64) -> bool {
        self.inner.perform(radius).unwrap_or(false)
    }

    pub fn result(&self) -> Option<EdgeTriple> {
        self.inner.result().ok().and_then(EdgeTriple::from_inner)
    }
}

impl Default for AnaFilletAlgo {
    fn default() -> Self {
        Self::new()
    }
}

/// `Part.ChFi2d.ChamferAPI` — the chamfer between two linear edges.
pub struct ChamferApi {
    inner: UniquePtr<chfi2d::ChFi2dChamferApi>,
}

impl ChamferApi {
    pub fn new() -> Self {
        ChamferApi {
            inner: chfi2d::chfi2d_chamfer_api_new(),
        }
    }

    pub fn init_wire(&self, wire: &Shape) {
        with_occt!(wire, |wire| {
            let _ = self.inner.init_wire(wire);
        });
    }

    pub fn init_edges(&self, edge1: &Shape, edge2: &Shape) {
        let (Ok(edge1), Ok(edge2)) = (downcast(edge1), downcast(edge2)) else {
            return;
        };
        let guard1 = edge1.borrow();
        let guard2 = edge2.borrow();
        let (Some(edge1), Some(edge2)) = (guard1.as_ref(), guard2.as_ref()) else {
            return;
        };
        let _ = self.inner.init_edges(edge1, edge2);
    }

    pub fn perform(&self) -> bool {
        self.inner.perform().unwrap_or(false)
    }

    pub fn result(&self, length1: f64, length2: f64) -> Option<EdgeTriple> {
        self.inner
            .result(length1, length2)
            .ok()
            .and_then(EdgeTriple::from_inner)
    }
}

impl Default for ChamferApi {
    fn default() -> Self {
        Self::new()
    }
}
