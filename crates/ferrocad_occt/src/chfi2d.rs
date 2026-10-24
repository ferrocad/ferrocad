//! The planar fillet and chamfer algorithms behind `Part.ChFi2d`.
//!
//! A sibling cxx bridge over `include/fc_chfi2d.hxx` (the same pattern as
//! [`crate::bridge`]): OCCT's `ChFi2d` package is not bridged by
//! `opencascade-sys`, so the header wraps `ChFi2d_FilletAlgo`,
//! `ChFi2d_AnaFilletAlgo` and `ChFi2d_ChamferAPI`. The algorithms are plain C++
//! classes owned by `unique_ptr`, so every bridge method is `const`.
//!
//! `perform()` on an uninitialised `FilletAlgo` raises inside OCCT; that is
//! reported as `None` here and becomes `Part.OCCError` (upstream's
//! `Base.CADKernelError`) in Python. A `false` result is a real "no fillet
//! found", not a failure.

use cxx::UniquePtr;
use ferrocad_geom::Shape;

use crate::shape::{downcast, erase};

#[cxx::bridge]
mod ffi {
    unsafe extern "C++" {
        include!("ferrocad_occt/include/fc_chfi2d.hxx");

        type TopoDS_Shape = opencascade_sys::topo_ds::TopoDS_Shape;

        type FcShapeTriple;
        type FcChFi2dFilletAlgo;
        type FcChFi2dAnaFilletAlgo;
        type FcChFi2dChamferAPI;

        fn fc_chfi2d_fillet_algo_new() -> UniquePtr<FcChFi2dFilletAlgo>;
        fn init_wire(
            self: &FcChFi2dFilletAlgo,
            wire: &TopoDS_Shape,
            ox: f64,
            oy: f64,
            oz: f64,
            nx: f64,
            ny: f64,
            nz: f64,
        );
        fn init_edges(
            self: &FcChFi2dFilletAlgo,
            edge1: &TopoDS_Shape,
            edge2: &TopoDS_Shape,
            ox: f64,
            oy: f64,
            oz: f64,
            nx: f64,
            ny: f64,
            nz: f64,
        );
        fn perform(self: &FcChFi2dFilletAlgo, radius: f64) -> i32;
        fn nb_results(self: &FcChFi2dFilletAlgo, x: f64, y: f64, z: f64) -> i32;
        fn result(
            self: &FcChFi2dFilletAlgo,
            x: f64,
            y: f64,
            z: f64,
            solution: i32,
        ) -> UniquePtr<FcShapeTriple>;

        fn fc_chfi2d_ana_fillet_algo_new() -> UniquePtr<FcChFi2dAnaFilletAlgo>;
        fn init_wire(
            self: &FcChFi2dAnaFilletAlgo,
            wire: &TopoDS_Shape,
            ox: f64,
            oy: f64,
            oz: f64,
            nx: f64,
            ny: f64,
            nz: f64,
        );
        fn init_edges(
            self: &FcChFi2dAnaFilletAlgo,
            edge1: &TopoDS_Shape,
            edge2: &TopoDS_Shape,
            ox: f64,
            oy: f64,
            oz: f64,
            nx: f64,
            ny: f64,
            nz: f64,
        );
        fn perform(self: &FcChFi2dAnaFilletAlgo, radius: f64) -> bool;
        fn result(self: &FcChFi2dAnaFilletAlgo) -> UniquePtr<FcShapeTriple>;

        fn fc_chfi2d_chamfer_api_new() -> UniquePtr<FcChFi2dChamferAPI>;
        fn init_wire(self: &FcChFi2dChamferAPI, wire: &TopoDS_Shape);
        fn init_edges(
            self: &FcChFi2dChamferAPI,
            edge1: &TopoDS_Shape,
            edge2: &TopoDS_Shape,
        );
        fn perform(self: &FcChFi2dChamferAPI) -> bool;
        fn result(
            self: &FcChFi2dChamferAPI,
            length1: f64,
            length2: f64,
        ) -> UniquePtr<FcShapeTriple>;

        fn first_shape(self: &FcShapeTriple) -> &TopoDS_Shape;
        fn second_shape(self: &FcShapeTriple) -> &TopoDS_Shape;
        fn third_shape(self: &FcShapeTriple) -> &TopoDS_Shape;
        fn first_is_null(self: &FcShapeTriple) -> bool;
        fn second_is_null(self: &FcShapeTriple) -> bool;
        fn third_is_null(self: &FcShapeTriple) -> bool;
    }
}

const _: () = ();

macro_rules! with_occt {
    ($shape:expr, |$inner:ident| $body:expr) => {{
        match downcast($shape) {
            Ok(occt) => {
                let guard = occt.borrow();
                match guard.as_ref() {
                    Some($inner) => $body,
                    None => Default::default(),
                }
            }
            Err(_) => Default::default(),
        }
    }};
}

/// The three edges an algorithm returns: the new fillet/chamfer, then the two
/// trimmed input edges.
pub struct EdgeTriple {
    inner: UniquePtr<ffi::FcShapeTriple>,
}

impl EdgeTriple {
    fn from_inner(inner: UniquePtr<ffi::FcShapeTriple>) -> Option<Self> {
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
    inner: UniquePtr<ffi::FcChFi2dFilletAlgo>,
}

impl FilletAlgo {
    pub fn new() -> Self {
        FilletAlgo {
            inner: ffi::fc_chfi2d_fillet_algo_new(),
        }
    }

    pub fn init_wire(&self, wire: &Shape, origin: [f64; 3], normal: [f64; 3]) {
        let _: () = with_occt!(wire, |wire| self.inner.init_wire(
            wire, origin[0], origin[1], origin[2], normal[0], normal[1], normal[2]
        ));
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
        self.inner.init_edges(
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
        match self.inner.perform(radius) {
            1 => Some(true),
            0 => Some(false),
            _ => None,
        }
    }

    pub fn number_of_results(&self, point: [f64; 3]) -> Option<i32> {
        let count = self.inner.nb_results(point[0], point[1], point[2]);
        (count >= 0).then_some(count)
    }

    pub fn result(&self, point: [f64; 3], solution: i32) -> Option<EdgeTriple> {
        EdgeTriple::from_inner(
            self.inner
                .result(point[0], point[1], point[2], solution),
        )
    }
}

impl Default for FilletAlgo {
    fn default() -> Self {
        Self::new()
    }
}

/// `Part.ChFi2d.AnaFilletAlgo` — the analytic (segment/arc) fillet.
pub struct AnaFilletAlgo {
    inner: UniquePtr<ffi::FcChFi2dAnaFilletAlgo>,
}

impl AnaFilletAlgo {
    pub fn new() -> Self {
        AnaFilletAlgo {
            inner: ffi::fc_chfi2d_ana_fillet_algo_new(),
        }
    }

    pub fn init_wire(&self, wire: &Shape, origin: [f64; 3], normal: [f64; 3]) {
        let _: () = with_occt!(wire, |wire| self.inner.init_wire(
            wire, origin[0], origin[1], origin[2], normal[0], normal[1], normal[2]
        ));
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
        self.inner.init_edges(
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
        self.inner.perform(radius)
    }

    pub fn result(&self) -> Option<EdgeTriple> {
        EdgeTriple::from_inner(self.inner.result())
    }
}

impl Default for AnaFilletAlgo {
    fn default() -> Self {
        Self::new()
    }
}

/// `Part.ChFi2d.ChamferAPI` — the chamfer between two linear edges.
pub struct ChamferApi {
    inner: UniquePtr<ffi::FcChFi2dChamferAPI>,
}

impl ChamferApi {
    pub fn new() -> Self {
        ChamferApi {
            inner: ffi::fc_chfi2d_chamfer_api_new(),
        }
    }

    pub fn init_wire(&self, wire: &Shape) {
        let _: () = with_occt!(wire, |wire| self.inner.init_wire(wire));
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
        self.inner.init_edges(edge1, edge2);
    }

    pub fn perform(&self) -> bool {
        self.inner.perform()
    }

    pub fn result(&self, length1: f64, length2: f64) -> Option<EdgeTriple> {
        EdgeTriple::from_inner(self.inner.result(length1, length2))
    }
}

impl Default for ChamferApi {
    fn default() -> Self {
        Self::new()
    }
}
