//! The planar fillet and chamfer algorithms behind `Part.ChFi2d`.
//!
//! The generic OCCT calls live in `opencascade_sys::chfi2d`; this module owns
//! the FerroCAD semantics: shape downcasting, `Option` mapping, and the
//! distinction between "no result" and "OCCT raised" that `Part.OCCError`
//! reports. The algorithms are plain C++ classes exposed as `UniquePtr`s, so a
//! `RefCell` supplies the `Pin<&mut _>` their `Init`/`Perform`/`Result` calls
//! need while the public methods stay `&self`.

use std::cell::RefCell;

use cxx::UniquePtr;
use ferrocad_geom::Shape;
use opencascade_sys as ffi;

use crate::shape::{downcast, erase};

use ffi::topo_ds::{TopoDS, TopoDS_Edge};

/// Build a `gp_Pln` from an origin point and a normal direction.
fn plane(origin: [f64; 3], normal: [f64; 3]) -> UniquePtr<ffi::gp::gp_Pln> {
    let point = ffi::gp::new_point(origin[0], origin[1], origin[2]);
    let direction = ffi::gp::gp_Dir_new(normal[0], normal[1], normal[2]);
    ffi::gp::gp_Pln_new(&point, &direction)
}

/// Wrap an owned `TopoDS_Edge` as a `Shape`, or `None` when the edge is null.
fn edge_shape(edge: &UniquePtr<TopoDS_Edge>) -> Option<Shape> {
    let edge = edge.as_ref()?;
    let shape = ffi::topo_ds::cast_edge_to_shape(edge);
    if shape.IsNull() {
        None
    } else {
        Some(erase(shape))
    }
}

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
    edge: UniquePtr<TopoDS_Edge>,
    first: UniquePtr<TopoDS_Edge>,
    second: UniquePtr<TopoDS_Edge>,
}

impl EdgeTriple {
    fn from_edges(
        edge: UniquePtr<TopoDS_Edge>,
        first: UniquePtr<TopoDS_Edge>,
        second: UniquePtr<TopoDS_Edge>,
    ) -> Self {
        Self { edge, first, second }
    }

    /// The newly built fillet or chamfer edge.
    pub fn edge(&self) -> Option<Shape> {
        edge_shape(&self.edge)
    }

    /// The first input edge, trimmed to its tangent/chamfer point.
    pub fn first(&self) -> Option<Shape> {
        edge_shape(&self.first)
    }

    /// The second input edge, trimmed to its tangent/chamfer point.
    pub fn second(&self) -> Option<Shape> {
        edge_shape(&self.second)
    }
}

/// `Part.ChFi2d.FilletAlgo` — the iterative `ChFi2d_FilletAlgo`.
pub struct FilletAlgo {
    inner: RefCell<UniquePtr<ffi::chfi2d::ChFi2d_FilletAlgo>>,
}

impl FilletAlgo {
    pub fn new() -> Self {
        FilletAlgo {
            inner: RefCell::new(ffi::chfi2d::ChFi2d_FilletAlgo_new()),
        }
    }

    pub fn init_wire(&self, wire: &Shape, origin: [f64; 3], normal: [f64; 3]) {
        with_occt!(wire, |wire| {
            let plane = plane(origin, normal);
            let _ = ffi::chfi2d::ChFi2d_FilletAlgo_Init(
                self.inner.borrow_mut().pin_mut(),
                TopoDS::Wire(wire),
                &plane,
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
        let plane = plane(origin, normal);
        let _ = ffi::chfi2d::ChFi2d_FilletAlgo_Init_edges(
            self.inner.borrow_mut().pin_mut(),
            TopoDS::Edge(edge1),
            TopoDS::Edge(edge2),
            &plane,
        );
    }

    /// `Some(true)` when a fillet was found, `Some(false)` when none was, and
    /// `None` when OCCT raised (for example on an uninitialised algorithm).
    pub fn perform(&self, radius: f64) -> Option<bool> {
        ffi::chfi2d::ChFi2d_FilletAlgo_Perform(self.inner.borrow_mut().pin_mut(), radius).ok()
    }

    pub fn number_of_results(&self, point: [f64; 3]) -> Option<i32> {
        let point = ffi::gp::new_point(point[0], point[1], point[2]);
        ffi::chfi2d::ChFi2d_FilletAlgo_NbResults(self.inner.borrow_mut().pin_mut(), &point).ok()
    }

    pub fn result(&self, point: [f64; 3], solution: i32) -> Option<EdgeTriple> {
        let point = ffi::gp::new_point(point[0], point[1], point[2]);
        let mut edge1 = ffi::topo_ds::TopoDS_Edge_new();
        let mut edge2 = ffi::topo_ds::TopoDS_Edge_new();
        let edge = ffi::chfi2d::ChFi2d_FilletAlgo_Result(
            self.inner.borrow_mut().pin_mut(),
            &point,
            solution,
            edge1.pin_mut(),
            edge2.pin_mut(),
        )
        .ok()?;
        Some(EdgeTriple::from_edges(edge, edge1, edge2))
    }
}

impl Default for FilletAlgo {
    fn default() -> Self {
        Self::new()
    }
}

/// `Part.ChFi2d.AnaFilletAlgo` — the analytic (segment/arc) fillet.
pub struct AnaFilletAlgo {
    inner: RefCell<UniquePtr<ffi::chfi2d::ChFi2d_AnaFilletAlgo>>,
}

impl AnaFilletAlgo {
    pub fn new() -> Self {
        AnaFilletAlgo {
            inner: RefCell::new(ffi::chfi2d::ChFi2d_AnaFilletAlgo_new()),
        }
    }

    pub fn init_wire(&self, wire: &Shape, origin: [f64; 3], normal: [f64; 3]) {
        with_occt!(wire, |wire| {
            let plane = plane(origin, normal);
            let _ = ffi::chfi2d::ChFi2d_AnaFilletAlgo_Init(
                self.inner.borrow_mut().pin_mut(),
                TopoDS::Wire(wire),
                &plane,
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
        let plane = plane(origin, normal);
        let _ = ffi::chfi2d::ChFi2d_AnaFilletAlgo_Init_edges(
            self.inner.borrow_mut().pin_mut(),
            TopoDS::Edge(edge1),
            TopoDS::Edge(edge2),
            &plane,
        );
    }

    pub fn perform(&self, radius: f64) -> bool {
        ffi::chfi2d::ChFi2d_AnaFilletAlgo_Perform(self.inner.borrow_mut().pin_mut(), radius)
            .unwrap_or(false)
    }

    pub fn result(&self) -> Option<EdgeTriple> {
        let mut edge1 = ffi::topo_ds::TopoDS_Edge_new();
        let mut edge2 = ffi::topo_ds::TopoDS_Edge_new();
        let edge = ffi::chfi2d::ChFi2d_AnaFilletAlgo_Result(
            self.inner.borrow_mut().pin_mut(),
            edge1.pin_mut(),
            edge2.pin_mut(),
        )
        .ok()?;
        Some(EdgeTriple::from_edges(edge, edge1, edge2))
    }
}

impl Default for AnaFilletAlgo {
    fn default() -> Self {
        Self::new()
    }
}

/// `Part.ChFi2d.ChamferAPI` — the chamfer between two linear edges.
pub struct ChamferApi {
    inner: RefCell<UniquePtr<ffi::chfi2d::ChFi2d_ChamferAPI>>,
}

impl ChamferApi {
    pub fn new() -> Self {
        ChamferApi {
            inner: RefCell::new(ffi::chfi2d::ChFi2d_ChamferAPI_new()),
        }
    }

    pub fn init_wire(&self, wire: &Shape) {
        with_occt!(wire, |wire| {
            let _ = ffi::chfi2d::ChFi2d_ChamferAPI_Init(
                self.inner.borrow_mut().pin_mut(),
                TopoDS::Wire(wire),
            );
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
        let _ = ffi::chfi2d::ChFi2d_ChamferAPI_Init_edges(
            self.inner.borrow_mut().pin_mut(),
            TopoDS::Edge(edge1),
            TopoDS::Edge(edge2),
        );
    }

    pub fn perform(&self) -> bool {
        ffi::chfi2d::ChFi2d_ChamferAPI_Perform(self.inner.borrow_mut().pin_mut()).unwrap_or(false)
    }

    pub fn result(&self, length1: f64, length2: f64) -> Option<EdgeTriple> {
        let mut edge1 = ffi::topo_ds::TopoDS_Edge_new();
        let mut edge2 = ffi::topo_ds::TopoDS_Edge_new();
        let edge = ffi::chfi2d::ChFi2d_ChamferAPI_Result(
            self.inner.borrow_mut().pin_mut(),
            length1,
            length2,
            edge1.pin_mut(),
            edge2.pin_mut(),
        )
        .ok()?;
        Some(EdgeTriple::from_edges(edge, edge1, edge2))
    }
}

impl Default for ChamferApi {
    fn default() -> Self {
        Self::new()
    }
}
