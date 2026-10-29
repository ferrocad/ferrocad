//! The OCCT implementation of [`ferrocad_geom::GeometryBackend`].
//!
//! First operations: [`make_box`](OcctBackend::make_box), fuse, cut, fillet and
//! place. Fuse/cut/common build a [`History`] from the sibling bridge's
//! `Modified`/`Generated`/`IsDeleted` for **both** operands and all three element
//! kinds the kernel reports (faces, edges, vertices), using the result's map of
//! the same kind to turn output sub-shapes into positional names. Each relation
//! also carries an [`ElementId`] (operand, kind, 1-based ordinal), so two operands
//! that both report a `Face1` remain distinguishable.
//!
//! Unchanged sub-shapes are deliberately **absent** from `modified`/`generated`:
//! an input that is neither modified, generated, nor deleted simply survives as
//! itself, and the mapper treats an unmapped, non-deleted input that way. (Verified
//! in `spikes/occt-history-rs`.) They are reported separately as `identity`
//! relations, and the result's own incidence graph adds `derived` naming steps, so
//! the reverse map names every result element upstream names.
//!
//! `cut` also reproduces upstream's compound policy
//! (`FCBRepAlgoAPI_BooleanOperation::Build`): a compound tool is flattened and
//! fused before the cut, and a compound base is cut child by child.

use cxx::UniquePtr;
use ferrocad_curves::{
    Arc, ArcOfHyperbola, ArcOfParabola, BezierCurve, Circle, EdgeCurve, Ellipse, EllipticalArc,
    Hyperbola, Line, NurbsCurve, OffsetCurve, Parabola, Vector3,
};
use ferrocad_geom::{
    BsplineSurfaceData, Capabilities, Continuity, ElementId, ElementKind, ElementRef, GeomError,
    GeometryBackend, History, Lineage, MapVersion, MassProperties, OpResult, OperandId,
    PrincipalProperties, Shape, ShapeDistance, SurfaceData, SurfaceSpec, Tessellation,
    TessellationOptions, ToleranceKind, unsupported,
};
use ferrocad_types::{Matrix4, Placement};
use opencascade_sys as ffi;

use std::collections::BTreeSet;

use ffi::top_abs::TopAbs_Orientation;
use ffi::top_abs::TopAbs_ShapeEnum;
use ffi::top_tools::TopTools_IndexedMapOfShape;
use ffi::top_tools::TopTools_ListOfShape;
use ffi::topo_ds::TopoDS_Shape;

use crate::bridge;
use crate::error::OcctError;
use crate::shape::{downcast, erase, new_map, sub_shape_map};
use crate::topology;

type Map = TopTools_IndexedMapOfShape;

/// A geometry kernel backed by OCCT.
#[derive(Debug, Default, Clone, Copy)]
pub struct OcctBackend;

impl OcctBackend {
    pub fn new() -> Self {
        OcctBackend
    }

    /// Name every face, edge and vertex of `shape` positionally.
    ///
    /// This is the element-map rule a *container* applies to itself: upstream's
    /// `TopoShape::makeElementCompound` calls `mapSubElement(shapes, op)`, which
    /// gives every sub-element of the compound a name although no operation
    /// related it to an input. A compound of two boxes therefore reports 52 names
    /// (2 × 26). The relation is an identity one — the name denotes the element it
    /// is — so it adds one reverse-map entry per element and no operand
    /// provenance.
    pub fn name_every_element(&self, shape: &Shape) -> History {
        let mut history = History::default();
        let Ok(occt) = downcast(shape) else {
            return history;
        };
        let guard = occt.borrow();
        let Some(inner) = guard.as_ref() else {
            return history;
        };
        // A default-constructed handle is non-null as a `UniquePtr` but holds a null
        // `TShape`; enumerating it would dereference that.
        if inner.IsNull() {
            return history;
        }
        for (kind, occt_kind) in HISTORY_KINDS {
            let map = sub_shape_map(inner, occt_kind);
            for ordinal in 1..=map.Extent() {
                let element = element_ref(kind, ordinal, map.FindKey(ordinal));
                history.push_identity(element.clone(), element);
            }
        }
        history
    }

    /// The element map a shape carries without an operation history.
    ///
    /// A **compound that holds a body** names its own sub-elements
    /// ([`name_every_element`](OcctBackend::name_every_element)), the mapping
    /// upstream's `makeElementCompound` performs. Every other shape carries none: a
    /// primitive and a single-body result are named by the operation that produced
    /// them, and a compound of lower elements (the wires a slice returns) is an
    /// internal container rather than a result body. When an operation does have a
    /// history, the caller folds that instead of this map.
    pub fn intrinsic_history(&self, shape: &Shape) -> Option<History> {
        let occt = downcast(shape).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        if inner.IsNull() {
            return None;
        }
        if inner.ShapeType() != TopAbs_ShapeEnum::TopAbs_COMPOUND {
            return None;
        }
        let solids = sub_shape_map(inner, TopAbs_ShapeEnum::TopAbs_SOLID);
        if solids.Extent() == 0 {
            return None;
        }
        Some(self.name_every_element(shape))
    }

    /// Drop the dangling internal wires a fused region may carry. A wire other
    /// than the outer boundary that encloses no area is the trace of a cutter
    /// that terminated inside the face, not a hole; removing it leaves the
    /// region with a single clean boundary.
    fn without_dangling_wires(&self, shape: &Shape) -> Shape {
        let Ok(occt) = downcast(shape) else {
            return shape.clone();
        };
        let guard = occt.borrow();
        let Some(inner) = guard.as_ref() else {
            return shape.clone();
        };
        if !bridge::fc_is_face(inner) {
            return shape.clone();
        }
        let wires = sub_shape_map(inner, TopAbs_ShapeEnum::TopAbs_WIRE);
        if wires.Extent() <= 1 {
            return shape.clone();
        }
        let Some(outer) = self.outer_wire(shape) else {
            return shape.clone();
        };
        let Ok(outer_occt) = downcast(&outer) else {
            return shape.clone();
        };
        let outer_guard = outer_occt.borrow();
        let Some(outer_inner) = outer_guard.as_ref() else {
            return shape.clone();
        };
        let outer_index = bridge::fc_indexed_map_find_index_same(&wires, outer_inner);

        // A wire bounds an area only when it encloses a region on its own; probe
        // each one and count the ones that do not.
        let area = |shape: &Shape| {
            self.mass_properties(shape)
                .map(|mass| mass.area)
                .unwrap_or(0.0)
        };
        let mut dangling = 0;
        for index in 1..=wires.Extent() {
            if index == outer_index {
                continue;
            }
            let wire = erase(wires.FindKey(index));
            let enclosed = {
                let Ok(wire_occt) = downcast(&wire) else {
                    return shape.clone();
                };
                let wire_guard = wire_occt.borrow();
                let Some(wire_inner) = wire_guard.as_ref() else {
                    return shape.clone();
                };
                match bridge::fc_split_wire_into_faces(wire_inner).as_ref() {
                    Some(enclosed) => {
                        let faces = sub_shape_map(enclosed, TopAbs_ShapeEnum::TopAbs_FACE);
                        (1..=faces.Extent())
                            .map(|face| area(&erase(faces.FindKey(face))))
                            .sum::<f64>()
                    }
                    None => 0.0,
                }
            };
            if enclosed <= 1e-9 {
                dangling += 1;
            }
        }
        // Real holes must be kept; only a face whose extra wires are all
        // dangling can be rebuilt from its outer boundary alone.
        if dangling == 0 || dangling != wires.Extent() - 1 {
            return shape.clone();
        }
        let Ok(cleaned) = self.make_face(&[outer]) else {
            return shape.clone();
        };
        let cleaned_area = area(&cleaned);
        let original_area = area(shape);
        if cleaned_area > 0.0
            && (cleaned_area - original_area).abs() <= 1e-9 * original_area.max(1.0)
        {
            cleaned
        } else {
            shape.clone()
        }
    }

    /// A B-spline edge from explicit poles, weights, unique knots and
    /// multiplicities (`Part.BSplineCurve.buildFromPolesMultsKnots`).
    ///
    /// The [`GeometryBackend`] seam's `make_bspline_poles` only knows the uniform
    /// clamped/periodic form (its arguments are poles, degree and the periodic
    /// flag), so the general knot vector has no seam entry point. This is the
    /// kernel's own constructor, used by the Part curve binding when the user
    /// supplies knots and multiplicities.
    #[allow(clippy::too_many_arguments)]
    pub fn make_bspline_poles_mults_knots(
        &self,
        poles: &[Vector3],
        weights: &[f64],
        knots: &[f64],
        multiplicities: &[i32],
        degree: i32,
        periodic: bool,
        check_rational: bool,
    ) -> Result<Shape, GeomError> {
        let mut coords = Vec::with_capacity(poles.len() * 3);
        for pole in poles {
            coords.extend_from_slice(&[pole.x, pole.y, pole.z]);
        }
        let edge = bridge::fc_bspline_edge(
            &coords,
            weights,
            knots,
            multiplicities,
            degree,
            periodic,
            check_rational,
        );
        let Some(edge) = edge.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "make_bspline_poles_mults_knots",
            }
            .into());
        };
        Ok(erase(edge))
    }

    /// The flattened state of the B-spline with the given poles, weights, knots
    /// and multiplicities, read back from OCCT (see
    /// [`make_bspline_poles_mults_knots`](Self::make_bspline_poles_mults_knots)).
    /// `None` when the arguments do not describe a valid curve.
    #[allow(clippy::too_many_arguments)]
    pub fn bspline_state(
        &self,
        poles: &[Vector3],
        weights: &[f64],
        knots: &[f64],
        multiplicities: &[i32],
        degree: i32,
        periodic: bool,
        check_rational: bool,
    ) -> Option<Vec<f64>> {
        let coords = flatten(poles);
        let state = bridge::fc_bspline_probe(
            &coords,
            weights,
            knots,
            multiplicities,
            degree,
            periodic,
            check_rational,
        );
        (!state.is_empty()).then_some(state)
    }

    /// The flattened state of the B-spline interpolating `points`
    /// (`Part.BSplineCurve.interpolate`), read back from OCCT. `None` when the
    /// interpolation fails.
    pub fn bspline_interpolated_state(
        &self,
        points: &[Vector3],
        periodic: bool,
    ) -> Option<Vec<f64>> {
        let state = bridge::fc_bspline_probe_interpolated(&flatten(points), periodic);
        (!state.is_empty()).then_some(state)
    }

    /// The state of the same curve after `SetPeriodic`/`SetNotPeriodic`. `None`
    /// when the conversion is rejected.
    #[allow(clippy::too_many_arguments)]
    pub fn bspline_periodic_state(
        &self,
        poles: &[Vector3],
        weights: &[f64],
        knots: &[f64],
        multiplicities: &[i32],
        degree: i32,
        periodic: bool,
        check_rational: bool,
        target_periodic: bool,
    ) -> Option<Vec<f64>> {
        let state = bridge::fc_bspline_set_periodic(
            &flatten(poles),
            weights,
            knots,
            multiplicities,
            degree,
            periodic,
            check_rational,
            target_periodic,
        );
        (!state.is_empty()).then_some(state)
    }

    /// `Geom_BSplineCurve::Resolution`: the parameter tolerance equivalent to
    /// `tolerance`. `None` when the curve cannot be built.
    #[allow(clippy::too_many_arguments)]
    pub fn bspline_resolution(
        &self,
        poles: &[Vector3],
        weights: &[f64],
        knots: &[f64],
        multiplicities: &[i32],
        degree: i32,
        periodic: bool,
        check_rational: bool,
        tolerance: f64,
    ) -> Option<f64> {
        let resolution = bridge::fc_bspline_resolution(
            &flatten(poles),
            weights,
            knots,
            multiplicities,
            degree,
            periodic,
            check_rational,
            tolerance,
        );
        resolution.is_finite().then_some(resolution)
    }

    /// `BSplineCurve.segment(u1, u2)`: the state of the curve restricted to
    /// `[u1, u2]` (`Geom_BSplineCurve::Segment`).
    #[allow(clippy::too_many_arguments)]
    pub fn bspline_segment(
        &self,
        poles: &[Vector3],
        weights: &[f64],
        knots: &[f64],
        multiplicities: &[i32],
        degree: i32,
        periodic: bool,
        check_rational: bool,
        u1: f64,
        u2: f64,
    ) -> Option<Vec<f64>> {
        let state = bridge::fc_bspline_segment(
            &flatten(poles),
            weights,
            knots,
            multiplicities,
            degree,
            periodic,
            check_rational,
            u1,
            u2,
        );
        (!state.is_empty()).then_some(state)
    }

    /// The point at parameter `u` on the B-spline with the given state
    /// (`BSplineCurve.value(u)`). `None` when the curve cannot be built or the
    /// parameter is outside its domain.
    #[allow(clippy::too_many_arguments)]
    pub fn bspline_point(
        &self,
        poles: &[Vector3],
        weights: &[f64],
        knots: &[f64],
        multiplicities: &[i32],
        degree: i32,
        periodic: bool,
        check_rational: bool,
        u: f64,
    ) -> Option<Vector3> {
        let point = bridge::fc_bspline_point(
            &flatten(poles),
            weights,
            knots,
            multiplicities,
            degree,
            periodic,
            check_rational,
            u,
        );
        (point.len() == 3).then(|| Vector3::new(point[0], point[1], point[2]))
    }

    /// A conic converted to a B-spline over `[first, last]`, flattened like
    /// [`bspline_state`](Self::bspline_state). `kind` is 2 for a circle and 3 for
    /// an ellipse; `x_axis` is the direction of parameter 0. With `nurbs` the
    /// exact rational conversion is used (`GeomConic::toNurbs`), otherwise the
    /// approximation of `GeomCurve::toBSpline`. `None` on failure.
    #[allow(clippy::too_many_arguments)]
    pub fn conic_spline_state(
        &self,
        kind: i32,
        center: Vector3,
        normal: Vector3,
        x_axis: Vector3,
        major: f64,
        minor: f64,
        first: f64,
        last: f64,
        nurbs: bool,
    ) -> Option<Vec<f64>> {
        let state = bridge::fc_conic_to_spline(
            kind, center.x, center.y, center.z, normal.x, normal.y, normal.z, x_axis.x, x_axis.y,
            x_axis.z, major, minor, first, last, nurbs,
        );
        (!state.is_empty()).then_some(state)
    }

    /// The curve parameter of the closest point to `point`
    /// (`GeomAPI_ProjectPointOnCurve`, `GeometryCurve.parameter`). `None` when
    /// the projection fails.
    #[allow(clippy::too_many_arguments)]
    pub fn bspline_parameter(
        &self,
        poles: &[Vector3],
        weights: &[f64],
        knots: &[f64],
        multiplicities: &[i32],
        degree: i32,
        periodic: bool,
        check_rational: bool,
        point: Vector3,
    ) -> Option<f64> {
        let parameter = bridge::fc_bspline_parameter(
            &flatten(poles),
            weights,
            knots,
            multiplicities,
            degree,
            periodic,
            check_rational,
            point.x,
            point.y,
            point.z,
        );
        parameter.is_finite().then_some(parameter)
    }

    /// The state of the B-spline with knot `index` reduced to multiplicity `mult`
    /// (`Geom_BSplineCurve::RemoveKnot`). `None` when the removal is rejected.
    #[allow(clippy::too_many_arguments)]
    pub fn bspline_remove_knot_state(
        &self,
        poles: &[Vector3],
        weights: &[f64],
        knots: &[f64],
        multiplicities: &[i32],
        degree: i32,
        periodic: bool,
        check_rational: bool,
        index: i32,
        mult: i32,
        tolerance: f64,
    ) -> Option<Vec<f64>> {
        let state = bridge::fc_bspline_remove_knot(
            &flatten(poles),
            weights,
            knots,
            multiplicities,
            degree,
            periodic,
            check_rational,
            index,
            mult,
            tolerance,
        );
        (!state.is_empty()).then_some(state)
    }

    /// The state of the B-spline approximating `points`
    /// (`GeomAPI_PointsToBSpline`), read back from OCCT. `None` when the
    /// approximation fails.
    pub fn bspline_approximated_state(
        &self,
        points: &[Vector3],
        degree_min: i32,
        degree_max: i32,
        tolerance: f64,
    ) -> Option<Vec<f64>> {
        let state = bridge::fc_bspline_approximated(
            &flatten(points),
            degree_min,
            degree_max,
            tolerance,
        );
        (!state.is_empty()).then_some(state)
    }

    /// The state of the B-spline with `u` inserted up to multiplicity `mult`
    /// (`Geom_BSplineCurve::InsertKnot`). `None` when the insertion is rejected.
    #[allow(clippy::too_many_arguments)]
    pub fn bspline_insert_knot_state(
        &self,
        poles: &[Vector3],
        weights: &[f64],
        knots: &[f64],
        multiplicities: &[i32],
        degree: i32,
        periodic: bool,
        check_rational: bool,
        u: f64,
        mult: i32,
        tolerance: f64,
    ) -> Option<Vec<f64>> {
        let state = bridge::fc_bspline_insert_knot(
            &flatten(poles),
            weights,
            knots,
            multiplicities,
            degree,
            periodic,
            check_rational,
            u,
            mult,
            tolerance,
        );
        (!state.is_empty()).then_some(state)
    }

    /// The surface normal of `face` at `(u, v)` (`BRepLProp_SLProps` through
    /// upstream's `Part::Tools::getNormal`), with the face orientation applied.
    /// `None` where the normal is not defined.
    pub fn face_normal_at(&self, face: &Shape, u: f64, v: f64) -> Option<Vector3> {
        let occt = downcast(face).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        if !bridge::fc_is_face(inner) {
            return None;
        }
        three(&bridge::fc_face_normal(inner, u, v))
    }

    /// The two unit surface tangents at `(u, v)` (`TangentU`, `TangentV`).
    pub fn face_tangents_at(
        &self,
        face: &Shape,
        u: f64,
        v: f64,
    ) -> Option<(Vector3, Vector3)> {
        let occt = downcast(face).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        if !bridge::fc_is_face(inner) {
            return None;
        }
        let raw = bridge::fc_face_tangents(inner, u, v);
        if raw.len() < 6 {
            return None;
        }
        Some((three(&raw[..3])?, three(&raw[3..6])?))
    }

    /// The two partial derivatives of order `order` (1 or 2) at `(u, v)`.
    pub fn face_derivatives_at(
        &self,
        face: &Shape,
        u: f64,
        v: f64,
        order: i32,
    ) -> Option<(Vector3, Vector3)> {
        let occt = downcast(face).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        if !bridge::fc_is_face(inner) {
            return None;
        }
        let raw = bridge::fc_face_derivatives(inner, u, v, order);
        if raw.len() < 6 {
            return None;
        }
        Some((three(&raw[..3])?, three(&raw[3..6])?))
    }

    /// The `(min, max)` principal curvatures of a face at `(u, v)`.
    pub fn face_curvature_at(&self, face: &Shape, u: f64, v: f64) -> Option<(f64, f64)> {
        let occt = downcast(face).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        if !bridge::fc_is_face(inner) {
            return None;
        }
        let raw = bridge::fc_face_curvature(inner, u, v);
        (raw.len() == 2).then(|| (raw[0], raw[1]))
    }

    /// Sample a wire into points (`Wire.discretize`, upstream's
    /// `BRepAdaptor_CompCurve` samplers). The modes are the same as
    /// [`GeometryBackend::edge_discretize`](ferrocad_geom::GeometryBackend::edge_discretize).
    #[allow(clippy::too_many_arguments)]
    pub fn wire_discretize(
        &self,
        wire: &Shape,
        mode: i32,
        value: f64,
        curvature: f64,
        minimum: i32,
        first: f64,
        last: f64,
    ) -> Result<Vec<Vector3>, GeomError> {
        let occt = downcast(wire)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null wire" })?;
        if !bridge::fc_is_wire(inner) {
            return Err(OcctError::InvalidInput {
                what: "not a wire",
            }
            .into());
        }
        let coordinates =
            bridge::fc_wire_discretize(inner, mode, value, curvature, minimum, first, last);
        if coordinates.is_empty() || coordinates.len() % 3 != 0 {
            return Err(OcctError::NotDone {
                operation: "wire_discretize",
            }
            .into());
        }
        Ok(coordinates
            .chunks_exact(3)
            .map(|point| Vector3::new(point[0], point[1], point[2]))
            .collect())
    }

    /// The untrimmed parameter bounds `(u1, u2, v1, v2)` of the surface behind a
    /// face (`Geom_Surface::Bounds`).
    pub fn face_surface_bounds(&self, face: &Shape) -> Option<(f64, f64, f64, f64)> {
        let occt = downcast(face).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        if !bridge::fc_is_face(inner) {
            return None;
        }
        let raw = bridge::fc_face_surface_bounds(inner);
        (raw.len() == 4).then(|| (raw[0], raw[1], raw[2], raw[3]))
    }

    /// The principal normal of an edge's curve at `parameter`.
    pub fn edge_normal_at(&self, edge: &Shape, parameter: f64) -> Option<Vector3> {
        let occt = downcast(edge).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        if !bridge::fc_is_edge(inner) {
            return None;
        }
        three(&bridge::fc_edge_normal(inner, parameter))
    }

    /// The derivative of order `order` (1, 2 or 3) of an edge's curve.
    pub fn edge_derivatives_at(
        &self,
        edge: &Shape,
        parameter: f64,
        order: i32,
    ) -> Option<Vector3> {
        let occt = downcast(edge).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        if !bridge::fc_is_edge(inner) {
            return None;
        }
        three(&bridge::fc_edge_derivatives(inner, parameter, order))
    }

    /// The curvature of an edge's curve at `parameter`.
    pub fn edge_curvature_at(&self, edge: &Shape, parameter: f64) -> Option<f64> {
        let occt = downcast(edge).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        if !bridge::fc_is_edge(inner) {
            return None;
        }
        let curvature = bridge::fc_edge_curvature(inner, parameter);
        curvature.is_finite().then_some(curvature)
    }

    /// The pcurve an edge has on a face, as a B-spline state (see
    /// [`SplineState`](ferrocad_part_py)) whose range is the edge's own.
    pub fn edge_pcurve_state(&self, edge: &Shape, face: &Shape) -> Option<Vec<f64>> {
        let edge = downcast(edge).ok()?;
        let face = downcast(face).ok()?;
        let edge = edge.borrow();
        let face = face.borrow();
        let state = bridge::fc_edge_pcurve(edge.as_ref()?, face.as_ref()?);
        (!state.is_empty()).then_some(state)
    }

    /// A 2-D B-spline lifted onto a surface as an edge: `poles` are `(u, v)`
    /// points, and the 3-D curve is the surface image of the pcurve
    /// (`BRepBuilderAPI_MakeEdge` with the pcurve/surface form).
    #[allow(clippy::too_many_arguments)]
    pub fn make_bspline2d_edge(
        &self,
        poles: &[(f64, f64)],
        weights: &[f64],
        knots: &[f64],
        multiplicities: &[i32],
        degree: i32,
        periodic: bool,
        surface: Option<&SurfaceSpec>,
        face: Option<&Shape>,
        range: Option<(f64, f64)>,
    ) -> Result<Shape, GeomError> {
        let mut coordinates = Vec::with_capacity(poles.len() * 2);
        for (u, v) in poles {
            coordinates.extend_from_slice(&[*u, *v]);
        }
        let spec = surface.map(surface_spec).unwrap_or_default();
        let owned_face = surface_face(face)?;
        // A default-constructed (null) face stands in when no surface was given;
        // the C++ side only looks at it when `spec` is empty.
        let null_face = ffi::topo_ds::TopoDS_Face_new();
        let face_shape: &TopoDS_Shape = match &owned_face {
            Some(shape) => shape,
            None => ffi::topo_ds::cast_face_to_shape(&null_face),
        };
        let (first, last) = range.unwrap_or((0.0, 0.0));
        let edge = bridge::fc_bspline2d_edge(
            &coordinates,
            weights,
            knots,
            multiplicities,
            degree,
            periodic,
            &spec,
            face_shape,
            first,
            last,
            range.is_some(),
        );
        match edge.as_ref() {
            Some(edge) => Ok(erase(edge)),
            None => Err(OcctError::NotDone {
                operation: "make_bspline2d_edge",
            }
            .into()),
        }
    }

    /// An analytic 2-D curve lifted onto a surface as an edge, upstream's
    /// `Curve2dPy::toShape(surface)` for the non-B-spline forms.
    ///
    /// `planar_edge` is the exact `z = 0` edge the caller already builds for the
    /// same 2-D curve; the kernel turns its curve into the pcurve and builds the
    /// surface image. The B-spline form goes through
    /// [`Self::make_bspline2d_edge`] instead, which carries the control points
    /// across directly.
    pub fn curve2d_on_surface_edge(
        &self,
        planar_edge: &Shape,
        surface: Option<&SurfaceSpec>,
        face: Option<&Shape>,
    ) -> Result<Shape, GeomError> {
        let spec = surface.map(surface_spec).unwrap_or_default();
        let owned_face = surface_face(face)?;
        let null_face = ffi::topo_ds::TopoDS_Face_new();
        let face_shape: &TopoDS_Shape = match &owned_face {
            Some(shape) => shape,
            None => ffi::topo_ds::cast_face_to_shape(&null_face),
        };
        let edge = downcast(planar_edge)?;
        let guard = edge.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null edge" })?;
        let lifted = bridge::fc_curve2d_on_surface(inner, &spec, face_shape);
        match lifted.as_ref() {
            Some(shape) => Ok(erase(shape)),
            None => Err(OcctError::NotDone {
                operation: "curve2d_on_surface_edge",
            }
            .into()),
        }
    }

    /// The face triangulations in **double** precision, as flat positions and
    /// per-face triangle indices (upstream's `Shape.tessellate`).
    ///
    /// [`GeometryBackend::tessellate`] narrows to `f32` for the viewport; the
    /// Python API promises `Base.Vector` precision, so it reads the same
    /// `Poly_Triangulation` nodes here without narrowing.
    pub fn tessellate_points(
        &self,
        shape: &Shape,
        options: &TessellationOptions,
    ) -> Result<(Vec<[f64; 3]>, Vec<u32>), GeomError> {
        crate::topology::tessellate_points(shape, options)
    }

    /// Assemble `faces` into one shell with `BRep_Builder`, without judging or
    /// repairing it.
    fn assemble_shell(&self, faces: &[Shape]) -> Result<Shape, GeomError> {
        if faces.is_empty() {
            return Err(OcctError::InvalidInput {
                what: "a shell needs at least one face",
            }
            .into());
        }
        let mut shell = ffi::topo_ds::TopoDS_Shell_new();
        let builder = ffi::b_rep::BRep_Builder_new();
        let builder = ffi::b_rep::BRep_Builder_upcast_to_topods_builder(&builder);
        builder.MakeShell(shell.pin_mut());
        let mut shell = ffi::topo_ds::TopoDS_Shell_as_shape(shell);
        for face in faces {
            let occt = downcast(face)?;
            let guard = occt.borrow();
            let inner = guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null face" })?;
            if !bridge::fc_is_face(inner) {
                return Err(OcctError::InvalidInput {
                    what: "a shell is made of faces",
                }
                .into());
            }
            builder.Add(shell.pin_mut(), inner);
        }
        Ok(erase(&shell))
    }
}

/// The owned surface face handed to the 2-D lifting bridges: `None` when the
/// caller passes an elementary surface spec instead.
fn surface_face(face: Option<&Shape>) -> Result<Option<UniquePtr<TopoDS_Shape>>, GeomError> {
    match face {
        Some(face) => {
            let occt = downcast(face)?;
            let guard = occt.borrow();
            let inner = guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null face" })?;
            Ok(Some(ffi::topo_ds::TopoDS_Shape_to_owned(inner)))
        }
        None => Ok(None),
    }
}

/// The `Vector3` in three leading components, or `None` when the kernel
/// reported nothing.
fn three(values: &[f64]) -> Option<Vector3> {
    (values.len() >= 3).then(|| Vector3::new(values[0], values[1], values[2]))
}

impl GeometryBackend for OcctBackend {
    fn make_box(&self, length: f64, width: f64, height: f64) -> Result<Shape, GeomError> {
        let origin = ffi::gp::new_point(0.0, 0.0, 0.0);
        let mut make = ffi::b_rep_prim_api::BRepPrimAPI_MakeBox_new(&origin, length, width, height);
        let shape = erase(make.pin_mut().Shape());
        if !make.IsDone() {
            return Err(OcctError::NotDone {
                operation: "make_box",
            }
            .into());
        }
        Ok(shape)
    }

    fn make_cylinder(
        &self,
        radius: f64,
        height: f64,
        base: Vector3,
        direction: Vector3,
    ) -> Result<Shape, GeomError> {
        let axis = axis_from(base, direction)?;
        let mut make = ffi::b_rep_prim_api::BRepPrimAPI_MakeCylinder_new(&axis, radius, height);
        let shape = erase(make.pin_mut().Shape());
        if !make.IsDone() {
            return Err(OcctError::NotDone {
                operation: "make_cylinder",
            }
            .into());
        }
        Ok(shape)
    }

    fn make_sphere(&self, radius: f64, center: Vector3) -> Result<Shape, GeomError> {
        let origin = point(center);
        let direction = ffi::gp::gp_Dir_new(0.0, 0.0, 1.0);
        let axis = ffi::gp::gp_Ax2_new(&origin, &direction);
        let mut make =
            ffi::b_rep_prim_api::BRepPrimAPI_MakeSphere_new(&axis, radius, std::f64::consts::TAU);
        let shape = erase(make.pin_mut().Shape());
        if !make.IsDone() {
            return Err(OcctError::NotDone {
                operation: "make_sphere",
            }
            .into());
        }
        Ok(shape)
    }

    fn make_cone(
        &self,
        radius1: f64,
        radius2: f64,
        height: f64,
        base: Vector3,
        direction: Vector3,
    ) -> Result<Shape, GeomError> {
        let axis = axis_from(base, direction)?;
        let mut make = ffi::b_rep_prim_api::BRepPrimAPI_MakeCone_new(
            &axis,
            radius1,
            radius2,
            height,
            std::f64::consts::TAU,
        );
        let shape = erase(make.pin_mut().Shape());
        if !make.IsDone() {
            return Err(OcctError::NotDone {
                operation: "make_cone",
            }
            .into());
        }
        Ok(shape)
    }

    fn make_edge(&self, curve: &EdgeCurve) -> Result<Shape, GeomError> {
        match curve {
            EdgeCurve::LineSegment(segment) => {
                let start = point(segment.start());
                let end = point(segment.end());
                let mut make =
                    ffi::b_rep_builder_api::BRepBuilderAPI_MakeEdge_gp_Pnt_gp_Pnt(&start, &end);
                if !make.IsDone() {
                    return Err(OcctError::NotDone {
                        operation: "make_edge",
                    }
                    .into());
                }
                Ok(erase(ffi::topo_ds::cast_edge_to_shape(
                    make.pin_mut().Edge(),
                )))
            }
            EdgeCurve::Circle(circle) => edge_from_circle(circle),
            EdgeCurve::Arc(arc) => {
                if arc.is_closed() {
                    // A full turn is a circle; `GC_MakeArcOfCircle` needs three
                    // distinct points, and the mid point would coincide.
                    return edge_from_circle(&arc.circle());
                }
                let start = point(arc.start());
                let middle = point(arc.point_at(arc.first() + arc.sweep() / 2.0));
                let end = point(arc.end());
                let arc_maker =
                    ffi::gc::GC_MakeArcOfCircle_point_point_point(&start, &middle, &end);
                let trimmed = ffi::gc::GC_MakeArcOfCircle_Value(&*arc_maker);
                if trimmed.is_null() {
                    return Err(OcctError::NotDone {
                        operation: "make_edge (arc)",
                    }
                    .into());
                }
                let curve = ffi::geom::new_HandleGeomCurve_from_HandleGeom_TrimmedCurve(&*trimmed);
                let mut make =
                    ffi::b_rep_builder_api::BRepBuilderAPI_MakeEdge_HandleGeomCurve(&*curve);
                if !make.IsDone() {
                    return Err(OcctError::NotDone {
                        operation: "make_edge (arc)",
                    }
                    .into());
                }
                Ok(erase(ffi::topo_ds::cast_edge_to_shape(
                    make.pin_mut().Edge(),
                )))
            }
            EdgeCurve::Line(_) => Err(unsupported("make_edge for an infinite line")),
            EdgeCurve::Ellipse(ellipse) => edge_from_ellipse(ellipse),
            EdgeCurve::EllipticalArc(arc) => {
                let ellipse = arc.ellipse();
                edge_from_conic(
                    3,
                    &ellipse,
                    arc.first(),
                    arc.last(),
                    "make_edge (elliptical arc)",
                )
            }
            EdgeCurve::Hyperbola(_) => Err(unsupported("make_edge for an unbounded hyperbola")),
            EdgeCurve::ArcOfHyperbola(arc) => {
                let hyperbola = arc.hyperbola();
                edge_from_conic(
                    4,
                    &hyperbola,
                    arc.first(),
                    arc.last(),
                    "make_edge (arc of hyperbola)",
                )
            }
            EdgeCurve::Parabola(_) => Err(unsupported("make_edge for an unbounded parabola")),
            EdgeCurve::ArcOfParabola(arc) => {
                let parabola = arc.parabola();
                edge_from_conic(
                    5,
                    &parabola,
                    arc.first(),
                    arc.last(),
                    "make_edge (arc of parabola)",
                )
            }
            EdgeCurve::BezierCurve(curve) => edge_from_bezier(curve),
            EdgeCurve::BSplineCurve(curve) => edge_from_nurbs(curve),
            EdgeCurve::OffsetCurve(curve) => {
                let basis = self.make_edge(curve.basis())?;
                let basis_occt = downcast(&basis)?;
                let basis_guard = basis_occt.borrow();
                let basis_inner = basis_guard
                    .as_ref()
                    .ok_or(OcctError::InvalidInput { what: "null basis edge" })?;
                let direction = curve.direction();
                let edge = bridge::fc_offset_curve_edge(
                    basis_inner,
                    curve.offset(),
                    direction.x,
                    direction.y,
                    direction.z,
                    curve.first(),
                    curve.last(),
                );
                let Some(edge) = edge.as_ref() else {
                    return Err(OcctError::NotDone {
                        operation: "make_edge (offset curve)",
                    }
                    .into());
                };
                Ok(erase(edge))
            }
        }
    }

    fn make_wire(&self, edges: &[Shape]) -> Result<Shape, GeomError> {
        let mut make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeWire_new();
        for edge in edges {
            let occt = downcast(edge)?;
            let guard = occt.borrow();
            let inner = guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null edge" })?;
            if !bridge::fc_is_edge(inner) {
                return Err(OcctError::InvalidInput {
                    what: "make_wire expects edges",
                }
                .into());
            }
            make.pin_mut().add_edge(bridge::fc_shape_as_edge(inner));
        }
        if !make.IsDone() {
            return Err(OcctError::NotDone {
                operation: "make_wire",
            }
            .into());
        }
        Ok(erase(ffi::topo_ds::cast_wire_to_shape(
            make.pin_mut().Wire(),
        )))
    }

    fn make_vertex(&self, at: Vector3) -> Result<Shape, GeomError> {
        let pnt = point(at);
        let mut make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeVertex_gp_Pnt(&pnt);
        Ok(erase(ffi::topo_ds::cast_vertex_to_shape(
            make.pin_mut().Vertex(),
        )))
    }

    fn vertex_point(&self, shape: &Shape) -> Option<Vector3> {
        let occt = downcast(shape).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        if !bridge::fc_is_vertex(inner) {
            return None;
        }
        let pnt = ffi::b_rep::BRep_Tool_Pnt(bridge::fc_shape_as_vertex(inner));
        Some(Vector3::new(pnt.X(), pnt.Y(), pnt.Z()))
    }

    fn make_bspline_edge(&self, points: &[Vector3], periodic: bool) -> Result<Shape, GeomError> {
        if points.len() < 2 {
            return Err(OcctError::InvalidInput {
                what: "a B-spline needs at least two points",
            }
            .into());
        }
        let mut array = ffi::t_col_gp::TColgp_HArray1OfPnt_new(1, points.len() as i32);
        for (index, value) in points.iter().enumerate() {
            let value = point(*value);
            array.pin_mut().SetValue(index as i32 + 1, &value);
        }
        let array = ffi::t_col_gp::new_HandleTColgpHArray1OfPnt_from_TColgpHArray1OfPnt(array);
        let mut interpolate = ffi::geom_api::GeomAPI_Interpolate_new(&array, periodic, 1e-7);
        interpolate.pin_mut().Perform();
        let curve = ffi::geom_api::GeomAPI_Interpolate_Curve(&interpolate);
        let curve = ffi::geom::new_HandleGeomCurve_from_HandleGeom_BSplineCurve(&curve);
        let mut make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeEdge_HandleGeomCurve(&curve);
        let shape = erase(ffi::topo_ds::cast_edge_to_shape(make.pin_mut().Edge()));
        if !make.IsDone() {
            return Err(OcctError::NotDone {
                operation: "make_bspline_edge",
            }
            .into());
        }
        Ok(shape)
    }

    fn make_bspline_poles(
        &self,
        poles: &[Vector3],
        degree: i32,
        periodic: bool,
    ) -> Result<Shape, GeomError> {
        if poles.len() < 2 {
            return Err(OcctError::InvalidInput {
                what: "a B-spline needs at least two poles",
            }
            .into());
        }
        let mut coords = Vec::with_capacity(poles.len() * 3);
        for pole in poles {
            coords.extend_from_slice(&[pole.x, pole.y, pole.z]);
        }
        let curve = bridge::fc_bspline_from_poles(&coords, degree, periodic);
        let Some(curve) = curve.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "make_bspline_poles",
            }
            .into());
        };
        let mut make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeEdge_HandleGeomCurve(curve);
        let shape = erase(ffi::topo_ds::cast_edge_to_shape(make.pin_mut().Edge()));
        if !make.IsDone() {
            return Err(OcctError::NotDone {
                operation: "make_bspline_poles",
            }
            .into());
        }
        Ok(shape)
    }

    fn make_face(&self, wires: &[Shape]) -> Result<Shape, GeomError> {
        let (first, rest) = wires.split_first().ok_or(OcctError::InvalidInput {
            what: "a face needs a wire",
        })?;
        let first_occt = downcast(first)?;
        let first_guard = first_occt.borrow();
        let first_inner = first_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null wire" })?;
        if !bridge::fc_is_wire(first_inner) {
            return Err(OcctError::InvalidInput {
                what: "make_face expects wires",
            }
            .into());
        }
        // Upstream's `BRepBuilderAPI_MakeFace(wire)`: the surface is found from
        // the wire itself, so a wire that lies on a cylinder or a sphere still
        // bounds a face. See `fc_face_from_wire` for the tolerance retry.
        let outer = bridge::fc_face_from_wire(first_inner);
        if outer.is_null() {
            return Err(OcctError::NotDone {
                operation: "make_face",
            }
            .into());
        }
        let mut face = outer;
        for wire in rest {
            let occt = downcast(wire)?;
            let guard = occt.borrow();
            let inner = guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null wire" })?;
            if !bridge::fc_is_wire(inner) {
                return Err(OcctError::InvalidInput {
                    what: "make_face expects wires",
                }
                .into());
            }
            // A hole must run opposite to the outer boundary; reverse it so the
            // boundary subtracts its area instead of adding another one.
            let reversed = bridge::fc_reversed(inner);
            let Some(reversed) = reversed.as_ref() else {
                return Err(OcctError::InvalidInput { what: "null wire" }.into());
            };
            let Some(source) = face.as_ref() else {
                return Err(OcctError::InvalidInput { what: "null face" }.into());
            };
            let next = bridge::fc_face_add_wire(source, reversed);
            if next.is_null() {
                return Err(OcctError::NotDone {
                    operation: "make_face",
                }
                .into());
            }
            face = next;
        }
        Ok(erase(&face))
    }

    fn make_faces(&self, wires: &[Shape]) -> Result<Shape, GeomError> {
        let mut faces = Vec::with_capacity(wires.len());
        let mut splitters = Vec::new();
        let mut all_planar = true;
        for wire in wires {
            let occt = downcast(wire)?;
            let guard = occt.borrow();
            let inner = guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null wire" })?;
            if !bridge::fc_is_wire(inner) {
                return Err(OcctError::InvalidInput {
                    what: "make_faces expects wires",
                }
                .into());
            }

            // A self-intersecting boundary splits into several loops (one face
            // per loop); a simple boundary stays on the ordinary path below.
            let split = bridge::fc_split_wire_into_faces(inner);
            if let Some(split) = split.as_ref() {
                let map = sub_shape_map(split, TopAbs_ShapeEnum::TopAbs_FACE);
                if map.Extent() > 1 {
                    for index in 1..=map.Extent() {
                        faces.push(erase(map.FindKey(index)));
                    }
                    continue;
                }
            }

            let make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeFace_wire(
                bridge::fc_shape_as_wire(inner),
                true,
            );
            if make.IsDone() {
                faces.push(erase(ffi::topo_ds::cast_face_to_shape(make.Face())));
                continue;
            }
            // A boundary lying on an existing surface (for example a cylinder's
            // lateral wire) can still build a face when the builder is allowed
            // to find that surface.
            let make_any = ffi::b_rep_builder_api::BRepBuilderAPI_MakeFace_wire(
                bridge::fc_shape_as_wire(inner),
                false,
            );
            if make_any.IsDone() {
                faces.push(erase(ffi::topo_ds::cast_face_to_shape(make_any.Face())));
                continue;
            }

            let mut list = ffi::top_tools::new_list_of_shape();
            let _ = list.pin_mut().Append(inner);
            let filled = bridge::fc_fill_wires(&list);
            let Some(filled) = filled.as_ref() else {
                // An open boundary cannot bound a face; it still subdivides the
                // faces around it (dangling ends are pruned by the boolean).
                splitters.push(wire.clone());
                continue;
            };
            let filled = erase(filled);
            // `BRepFill_Filling` can also "succeed" on an open boundary, but only
            // with a degenerate face. Such a wire is a cutter, not a boundary.
            let filled_area = self
                .mass_properties(&filled)
                .map(|mass| mass.area)
                .unwrap_or(0.0);
            if filled_area <= 1e-9 {
                splitters.push(wire.clone());
                continue;
            }
            all_planar = false;
            faces.push(filled);
        }

        if !splitters.is_empty() {
            if faces.is_empty() {
                return Err(OcctError::InvalidInput {
                    what: "an open wire cannot form a face",
                }
                .into());
            }
            let mut list = ffi::top_tools::new_list_of_shape();
            for shape in faces.iter().chain(splitters.iter()) {
                let occt = downcast(shape)?;
                let guard = occt.borrow();
                if let Some(inner) = guard.as_ref() {
                    let _ = list.pin_mut().Append(inner);
                }
            }
            let fused = bridge::fc_general_fuse(&list);
            if let Some(fused) = fused.as_ref() {
                let map = sub_shape_map(fused, TopAbs_ShapeEnum::TopAbs_FACE);
                let mut regions = Vec::with_capacity(map.Extent() as usize);
                for index in 1..=map.Extent() {
                    let region = erase(map.FindKey(index));
                    // General Fuse keeps the degenerate slivers and the dangling
                    // cutters as regions of their own; neither bounds an area.
                    let area = self
                        .mass_properties(&region)
                        .map(|mass| mass.area)
                        .unwrap_or(0.0);
                    if area <= 1e-9 {
                        continue;
                    }
                    regions.push(self.without_dangling_wires(&region));
                }
                if !regions.is_empty() {
                    return if regions.len() == 1 {
                        Ok(regions.remove(0))
                    } else {
                        self.make_compound(&regions)
                    };
                }
            }
            return Err(OcctError::NotDone {
                operation: "make_faces",
            }
            .into());
        }

        // Coplanar faces that overlap or cross must be split into their regions;
        // General Fuse returns all parts, unlike a boolean union. Nested boundaries
        // are different: upstream keeps them as one face with holes.
        if all_planar && faces.len() > 1 {
            let area = |shape: &Shape| {
                self.mass_properties(shape)
                    .map(|mass| mass.area)
                    .unwrap_or(0.0)
            };
            let contains = |outer: &Shape, inner: &Shape| {
                let inner_area = area(inner);
                if inner_area <= 0.0 {
                    return false;
                }
                match self.common(outer, inner) {
                    Ok(result) => {
                        let overlap = area(&result.shape);
                        (overlap - inner_area).abs() <= 1e-6 * inner_area.max(1.0)
                    }
                    Err(_) => false,
                }
            };
            // Nesting follows the even-odd rule: the outermost boundary is a face,
            // its direct children are holes, their children are islands, and so on.
            let count = faces.len();
            let mut parent: Vec<Option<usize>> = vec![None; count];
            for inner in 0..count {
                for outer in 0..count {
                    if inner == outer || !contains(&faces[outer], &faces[inner]) {
                        continue;
                    }
                    let smaller = parent[inner]
                        .map(|current| area(&faces[outer]) < area(&faces[current]))
                        .unwrap_or(true);
                    if smaller {
                        parent[inner] = Some(outer);
                    }
                }
            }
            if parent.iter().any(Option::is_some) {
                let depth = |mut index: usize| {
                    let mut depth = 0;
                    while let Some(up) = parent[index] {
                        depth += 1;
                        index = up;
                        if depth > count {
                            break;
                        }
                    }
                    depth
                };
                let even: Vec<usize> = (0..count).filter(|index| depth(*index) % 2 == 0).collect();
                let odd: Vec<usize> = (0..count).filter(|index| depth(*index) % 2 == 1).collect();
                let intersection_area = |a: &Shape, b: &Shape| match self.common(a, b) {
                    Ok(result) => area(&result.shape),
                    Err(_) => 0.0,
                };

                // Positive faces that partially overlap must be unioned first; the
                // hole regions are then removed from the split result.
                let mut partial_overlap = false;
                'pairs: for (position, &a) in even.iter().enumerate() {
                    for &b in &even[position + 1..] {
                        let overlap = intersection_area(&faces[a], &faces[b]);
                        let smaller = area(&faces[a]).min(area(&faces[b]));
                        if overlap > 1e-9 && smaller - overlap > 1e-9 {
                            partial_overlap = true;
                            break 'pairs;
                        }
                    }
                }
                if !odd.is_empty() && partial_overlap {
                    // Include the hole boundaries too: General Fuse must split on
                    // them so there is a region inside each hole to discard.
                    let mut list = ffi::top_tools::new_list_of_shape();
                    for face in &faces {
                        let occt = downcast(face)?;
                        let guard = occt.borrow();
                        if let Some(inner) = guard.as_ref() {
                            let _ = list.pin_mut().Append(inner);
                        }
                    }
                    let fused = bridge::fc_general_fuse(&list);
                    if let Some(fused) = fused.as_ref() {
                        let map = sub_shape_map(fused, TopAbs_ShapeEnum::TopAbs_FACE);
                        let mut kept = Vec::new();
                        for index in 1..=map.Extent() {
                            let region = erase(map.FindKey(index));
                            let inside_hole =
                                odd.iter().any(|&hole| contains(&faces[hole], &region));
                            if !inside_hole {
                                kept.push(region);
                            }
                        }
                        if !kept.is_empty() {
                            return if kept.len() == 1 {
                                Ok(kept.remove(0))
                            } else {
                                self.make_compound(&kept)
                            };
                        }
                    }
                }

                let mut built = Vec::new();
                for node in 0..count {
                    if depth(node) % 2 != 0 {
                        continue;
                    }
                    let mut boundaries = vec![wires[node].clone()];
                    let mut holes: Vec<(f64, Shape)> = (0..count)
                        .filter(|&child| parent[child] == Some(node))
                        .map(|child| (area(&faces[child]), wires[child].clone()))
                        .collect();
                    holes
                        .sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
                    boundaries.extend(holes.into_iter().map(|(_, wire)| wire));
                    if let Ok(face) = self.make_face(&boundaries) {
                        built.push(face);
                    }
                }
                if !built.is_empty() {
                    return if built.len() == 1 {
                        Ok(built.remove(0))
                    } else {
                        self.make_compound(&built)
                    };
                }
            }

            let mut list = ffi::top_tools::new_list_of_shape();
            for face in &faces {
                let occt = downcast(face)?;
                let guard = occt.borrow();
                if let Some(inner) = guard.as_ref() {
                    let _ = list.pin_mut().Append(inner);
                }
            }
            let fused = bridge::fc_general_fuse(&list);
            if let Some(fused) = fused.as_ref() {
                let map = sub_shape_map(fused, TopAbs_ShapeEnum::TopAbs_FACE);
                let mut regions = Vec::with_capacity(map.Extent() as usize);
                for index in 1..=map.Extent() {
                    regions.push(erase(map.FindKey(index)));
                }
                if regions.len() > 1 {
                    return self.make_compound(&regions);
                }
            }
        }
        match faces.len() {
            0 => Err(OcctError::InvalidInput {
                what: "a face needs a wire",
            }
            .into()),
            1 => Ok(faces.remove(0)),
            _ => self.make_compound(&faces),
        }
    }

    fn make_face_on_surface(
        &self,
        surface: &SurfaceSpec,
        wires: &[Shape],
    ) -> Result<Shape, GeomError> {
        if wires.is_empty() {
            return Err(OcctError::InvalidInput {
                what: "a face needs a wire",
            }
            .into());
        }
        let mut list = ffi::top_tools::new_list_of_shape();
        for wire in wires {
            let occt = downcast(wire)?;
            let guard = occt.borrow();
            let inner = guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null wire" })?;
            if !bridge::fc_is_wire(inner) {
                return Err(OcctError::InvalidInput {
                    what: "make_face_on_surface expects wires",
                }
                .into());
            }
            let _ = list.pin_mut().Append(inner);
        }
        let spec = surface_spec(surface);
        let face = bridge::fc_make_face_on_surface(&spec, &list);
        match face.as_ref() {
            Some(face) => Ok(erase(face)),
            None => Err(OcctError::NotDone {
                operation: "make_face_on_surface",
            }
            .into()),
        }
    }

    fn surface_type(&self, shape: &Shape) -> Option<String> {
        let occt = downcast(shape).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        if !bridge::fc_is_face(inner) {
            return None;
        }
        let surface = ffi::b_rep::BRep_Tool_Surface(bridge::fc_shape_as_face(inner));
        let name = ffi::standard::type_name(ffi::geom::DynamicType(surface.as_ref()?));
        Some(
            match name.as_str() {
                "Geom_Plane" => "Part::GeomPlane",
                "Geom_CylindricalSurface" => "Part::GeomCylinder",
                "Geom_ConicalSurface" => "Part::GeomCone",
                "Geom_SphericalSurface" => "Part::GeomSphere",
                "Geom_ToroidalSurface" => "Part::GeomToroid",
                "Geom_BSplineSurface" => "Part::GeomBSplineSurface",
                _ => "Part::GeomSurface",
            }
            .to_string(),
        )
    }

    fn surface_data(&self, shape: &Shape) -> Option<SurfaceData> {
        let occt = downcast(shape).ok()?;
        let mut raw = Vec::new();
        {
            let guard = occt.borrow();
            let inner = guard.as_ref()?;
            if !bridge::fc_is_face(inner) {
                return None;
            }
            bridge::fc_face_surface_frame(inner, &mut raw);
        }
        let point = |at: usize| Vector3::new(raw[at], raw[at + 1], raw[at + 2]);
        match raw.first().copied() {
            Some(1.0) if raw.len() >= 14 => Some(SurfaceData::Plane {
                origin: point(1),
                x_dir: point(4),
                y_dir: point(7),
                normal: point(10),
            }),
            Some(2.0) if raw.len() >= 12 => Some(SurfaceData::Cylinder {
                origin: point(1),
                axis: point(4),
                x_dir: point(7),
                radius: raw[10],
                direct: raw[11] > 0.5,
            }),
            Some(3.0) if raw.len() >= 12 => Some(SurfaceData::Sphere {
                center: point(1),
                axis: point(4),
                x_dir: point(7),
                radius: raw[10],
                direct: raw[11] > 0.5,
            }),
            Some(4.0) if raw.len() >= 13 => Some(SurfaceData::Cone {
                origin: point(1),
                axis: point(4),
                x_dir: point(7),
                radius: raw[10],
                semi_angle: raw[11],
                direct: raw[12] > 0.5,
            }),
            Some(5.0) if raw.len() >= 13 => Some(SurfaceData::Torus {
                center: point(1),
                axis: point(4),
                x_dir: point(7),
                major_radius: raw[10],
                minor_radius: raw[11],
                direct: raw[12] > 0.5,
            }),
            _ => self
                .surface_type(shape)
                .map(|type_id| SurfaceData::Other { type_id }),
        }
    }

    fn sew(&self, shape: &Shape, tolerance: f64) -> Result<Shape, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let sewn = bridge::fc_sew_shape(inner, tolerance);
        match sewn.as_ref() {
            Some(sewn) => Ok(erase(sewn)),
            None => Err(OcctError::NotDone { operation: "sew" }.into()),
        }
    }

    fn to_nurbs(&self, shape: &Shape) -> Result<Shape, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let converted = bridge::fc_to_nurbs(inner);
        match converted.as_ref() {
            Some(converted) => Ok(erase(converted)),
            None => Err(OcctError::NotDone {
                operation: "to_nurbs",
            }
            .into()),
        }
    }

    fn sub_shapes_of_type(&self, shape: &Shape, kind: &str) -> Vec<Shape> {
        let Ok(occt) = downcast(shape) else {
            return Vec::new();
        };
        let guard = occt.borrow();
        let Some(inner) = guard.as_ref() else {
            return Vec::new();
        };
        // `TopAbs_COMPOUND` is 0 and `TopAbs_COMPSOLID` is 1 in OCCT's
        // `TopAbs_ShapeEnum`; the cxx enum is not castable from Rust.
        let shape_type = match kind {
            "Compound" => 0,
            "CompSolid" => 1,
            _ => return Vec::new(),
        };
        let Some(compound) = bridge::fc_shape_sub_shapes_of_type(inner, shape_type)
            .as_ref()
            .map(erase)
        else {
            return Vec::new();
        };
        // `fc_shape_sub_shapes_of_type` wraps the map in a compound so it can
        // cross the bridge; its direct children are the sub-shapes themselves.
        let compound_occt = match downcast(&compound) {
            Ok(occt) => occt,
            Err(_) => return Vec::new(),
        };
        let compound_guard = compound_occt.borrow();
        let Some(compound_inner) = compound_guard.as_ref() else {
            return Vec::new();
        };
        let count = bridge::fc_shape_child_count(compound_inner);
        (1..=count)
            .filter_map(|index| bridge::fc_shape_child(compound_inner, index).as_ref().map(erase))
            .collect()
    }

    fn outer_wire(&self, shape: &Shape) -> Option<Shape> {
        let occt = downcast(shape).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        if !bridge::fc_is_face(inner) {
            return None;
        }
        let wire = ffi::b_rep_tools::outer_wire(bridge::fc_shape_as_face(inner));
        Some(erase(ffi::topo_ds::cast_wire_to_shape(wire.as_ref()?)))
    }

    fn curve_of(&self, shape: &Shape) -> Result<EdgeCurve, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let data = bridge::fc_edge_curve(inner);
        if data.len() < 14 {
            return Err(unsupported("curve_of: the bridge returned no curve data"));
        }
        parse_edge_curve(&data)
    }

    fn fuse(&self, a: &Shape, b: &Shape) -> Result<OpResult, GeomError> {
        let a_occt = downcast(a)?;
        let b_occt = downcast(b)?;
        let a_guard = a_occt.borrow();
        let b_guard = b_occt.borrow();
        let sa = a_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let sb = b_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let mut op = ffi::b_rep_algo_api::BRepAlgoAPI_Fuse_new(sa, sb);
        if !op.IsDone() {
            return Err(OcctError::NotDone { operation: "fuse" }.into());
        }
        let out_maps = sub_shape_maps(op.pin_mut().Shape());
        let history = build_history(
            [(OperandId::A, sa), (OperandId::B, sb)],
            &out_maps,
            |sub, mods, gens| {
                bridge::fc_brep_fuse_modified(op.pin_mut(), sub, mods.pin_mut());
                bridge::fc_brep_fuse_generated(op.pin_mut(), sub, gens.pin_mut());
                bridge::fc_brep_fuse_is_deleted(op.pin_mut(), sub)
            },
        );
        Ok(OpResult::new(erase(op.pin_mut().Shape()), history))
    }

    fn cut(&self, a: &Shape, b: &Shape) -> Result<OpResult, GeomError> {
        let a_occt = downcast(a)?;
        let b_occt = downcast(b)?;
        let a_guard = a_occt.borrow();
        let b_guard = b_occt.borrow();
        let sa = a_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let sb = b_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;

        // Upstream's `FCBRepAlgoAPI_BooleanOperation::Build` rewrites a compound
        // cut before OCCT sees it. One operation object is reused for every
        // internal build, so its history describes the last of them — which is
        // exactly what the element map is built from.
        let mut op = bridge::fc_cut_new_op();
        // The constructor-time settings, taken from the *original* operands: the
        // fuzzy value is not recomputed for the internal fuse/cut builds.
        let initial_arguments = shape_list(&[sa]);
        let initial_tools = shape_list(&[sb]);
        bridge::fc_cut_configure(op.pin_mut(), &initial_arguments, &initial_tools);
        bridge::fc_cut_prepare(op.pin_mut());
        bridge::fc_cut_set_auto_fuzzy(op.pin_mut());

        let Some(result) = build_cut_policy(&mut op, &[sa], &[sb], CutOperation::Cut) else {
            return Err(OcctError::NotDone { operation: "cut" }.into());
        };

        let result = result.as_ref().expect("the cut result");
        let out_maps = sub_shape_maps(result);
        let history = build_history(
            [(OperandId::A, sa), (OperandId::B, sb)],
            &out_maps,
            |sub, mods, gens| {
                bridge::fc_brep_cut_modified(op.pin_mut(), sub, mods.pin_mut());
                bridge::fc_brep_cut_generated(op.pin_mut(), sub, gens.pin_mut());
                bridge::fc_brep_cut_is_deleted(op.pin_mut(), sub)
            },
        );
        Ok(OpResult::new(erase(result), history))
    }

    fn fillet(
        &self,
        shape: &Shape,
        edges: &[ElementRef],
        radius: f64,
    ) -> Result<OpResult, GeomError> {
        topology::fillet(shape, edges, radius)
    }

    fn common(&self, a: &Shape, b: &Shape) -> Result<OpResult, GeomError> {
        let a_occt = downcast(a)?;
        let b_occt = downcast(b)?;
        let a_guard = a_occt.borrow();
        let b_guard = b_occt.borrow();
        let sa = a_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let sb = b_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let mut op = ffi::b_rep_algo_api::BRepAlgoAPI_Common_new(sa, sb);
        if !op.IsDone() {
            return Err(OcctError::NotDone {
                operation: "common",
            }
            .into());
        }
        let out_maps = sub_shape_maps(op.pin_mut().Shape());
        let history = build_history(
            [(OperandId::A, sa), (OperandId::B, sb)],
            &out_maps,
            |sub, mods, gens| {
                bridge::fc_brep_common_modified(op.pin_mut(), sub, mods.pin_mut());
                bridge::fc_brep_common_generated(op.pin_mut(), sub, gens.pin_mut());
                bridge::fc_brep_common_is_deleted(op.pin_mut(), sub)
            },
        );
        Ok(OpResult::new(erase(op.pin_mut().Shape()), history))
    }

    fn make_compound(&self, shapes: &[Shape]) -> Result<Shape, GeomError> {
        if shapes.is_empty() {
            return Err(OcctError::InvalidInput {
                what: "a compound needs at least one shape",
            }
            .into());
        }
        let mut compound = ffi::topo_ds::TopoDS_Compound_new();
        let builder = ffi::b_rep::BRep_Builder_new();
        let builder = ffi::b_rep::BRep_Builder_upcast_to_topods_builder(&builder);
        builder.MakeCompound(compound.pin_mut());

        // `TopoDS_Builder::Add(S, C)` takes the container first and the child
        // second; the cxx binding keeps that order (`shape` is the compound).
        let mut compound = ffi::topo_ds::TopoDS_Compound_as_shape(compound);
        for shape in shapes {
            let occt = downcast(shape)?;
            let guard = occt.borrow();
            let inner = guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null shape" })?;
            builder.Add(compound.pin_mut(), inner);
        }
        Ok(erase(&compound))
    }

    fn make_comp_solid(&self, solids: &[Shape]) -> Result<Shape, GeomError> {
        if solids.is_empty() {
            return Err(OcctError::InvalidInput {
                what: "a comp-solid needs at least one solid",
            }
            .into());
        }
        let mut list = ffi::top_tools::new_list_of_shape();
        for solid in solids {
            let occt = downcast(solid)?;
            let guard = occt.borrow();
            let inner = guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null solid" })?;
            let _ = list.pin_mut().Append(inner);
        }
        let composed = bridge::fc_comp_solid(&list);
        let Some(composed) = composed.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "make_comp_solid",
            }
            .into());
        };
        Ok(erase(composed))
    }

    fn make_solid(&self, shell: &Shape) -> Result<Shape, GeomError> {
        let occt = downcast(shell)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shell" })?;
        if !bridge::fc_is_shell(inner) {
            return Err(OcctError::InvalidInput {
                what: "make_solid expects a shell",
            }
            .into());
        }
        let mut make =
            ffi::b_rep_builder_api::BRepBuilderAPI_MakeSolid_new(bridge::fc_shape_as_shell(inner));
        let shape = erase(make.pin_mut().Shape());
        if !make.IsDone() {
            return Err(OcctError::NotDone {
                operation: "make_solid",
            }
            .into());
        }
        Ok(shape)
    }

    fn make_shell(&self, faces: &[Shape]) -> Result<Shape, GeomError> {
        let shell = self.assemble_shell(faces)?;
        // Upstream's `makeElementBoolean(OpCodes::Shell)` re-sews the assembly
        // when the naive `BRep_Builder` shell is not a valid shell. Independently
        // built faces (a mesh's triangles, a set of planar regions) only share
        // geometry, not topology, so without this they never form a closed
        // shell.
        if self.is_valid(&shell) {
            return Ok(shell);
        }
        let occt = downcast(&shell)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shell" })?;
        match bridge::fc_sew_shell(inner).as_ref() {
            Some(sewn) => Ok(erase(sewn)),
            None => Ok(shell),
        }
    }

    fn make_shell_from_wires(&self, wires: &[Shape]) -> Result<Shape, GeomError> {
        if wires.is_empty() {
            return Err(OcctError::InvalidInput {
                what: "a shell needs at least one wire",
            }
            .into());
        }
        let mut faces = Vec::with_capacity(wires.len());
        for wire in wires {
            // Each wire bounds one planar face; upstream's wires are the
            // boundaries of a box's faces.
            faces.push(self.make_face(std::slice::from_ref(wire))?);
        }
        // No sewing here: this entry point assembles the faces it just made, and
        // upstream's `makeElementShellFromWires` does not run the shell sewing
        // `Part.makeShell` falls back to.
        self.assemble_shell(&faces)
    }

    fn make_loft(&self, wires: &[Shape], solid: bool) -> Result<Shape, GeomError> {
        if wires.len() < 2 {
            return Err(OcctError::InvalidInput {
                what: "a loft needs at least two sections",
            }
            .into());
        }
        let mut make = ffi::b_rep_offset_api::BRepOffsetAPI_ThruSections_new(solid);
        for wire in wires {
            let occt = downcast(wire)?;
            let guard = occt.borrow();
            let inner = guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null wire" })?;
            if !bridge::fc_is_wire(inner) {
                return Err(OcctError::InvalidInput {
                    what: "make_loft expects wires",
                }
                .into());
            }
            make.pin_mut().AddWire(bridge::fc_shape_as_wire(inner));
        }
        // Upstream keeps the wires in the order given rather than letting OCCT
        // reorder them to match a common origin.
        make.pin_mut().CheckCompatibility(false);
        let progress = ffi::message::Message_ProgressRange_new();
        make.pin_mut().Build(&progress);
        if !make.IsDone() {
            return Err(OcctError::NotDone {
                operation: "make_loft",
            }
            .into());
        }
        Ok(erase(make.pin_mut().Shape()))
    }

    fn make_ruled_surface(&self, first: &Shape, second: &Shape) -> Result<Shape, GeomError> {
        let first_occt = downcast(first)?;
        let second_occt = downcast(second)?;
        let first_guard = first_occt.borrow();
        let second_guard = second_occt.borrow();
        let a = first_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null edge" })?;
        let b = second_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null edge" })?;
        let face = bridge::fc_ruled_surface(a, b);
        let Some(face) = face.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "make_ruled_surface",
            }
            .into());
        };
        Ok(erase(face))
    }

    fn make_pipe(
        &self,
        spine: &Shape,
        profile: &Shape,
        tolerance: f64,
    ) -> Result<Shape, GeomError> {
        let spine_occt = downcast(spine)?;
        let spine_guard = spine_occt.borrow();
        let spine_inner = spine_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null spine" })?;
        let profile_occt = downcast(profile)?;
        let profile_guard = profile_occt.borrow();
        let profile_inner = profile_guard.as_ref().ok_or(OcctError::InvalidInput {
            what: "null profile",
        })?;
        let piped = bridge::fc_pipe(spine_inner, profile_inner, tolerance);
        let Some(piped) = piped.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "make_pipe",
            }
            .into());
        };
        Ok(erase(piped))
    }

    fn intrinsic_lineage(&self, shape: &Shape) -> Result<Lineage, GeomError> {
        // A container that holds a body keeps the map `intrinsic_history` already
        // describes; a shell, solid, loft or filled face names every result
        // element, which is the map the factory attaches upstream. Callers decide
        // where the count is known to match.
        let history = self
            .intrinsic_history(shape)
            .unwrap_or_else(|| self.name_every_element(shape));
        Ok(Lineage::from_history(&history, MapVersion::default()))
    }

    fn make_filled_face(&self, edges: &[Shape]) -> Result<Shape, GeomError> {
        let wire = self.make_wire(edges)?;
        let mut list = ffi::top_tools::new_list_of_shape();
        {
            let occt = downcast(&wire)?;
            let guard = occt.borrow();
            let inner = guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null wire" })?;
            list.pin_mut().Append(inner);
        }
        let face = bridge::fc_fill_wires(&list);
        let Some(face) = face.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "make_filled_face",
            }
            .into());
        };
        Ok(erase(face))
    }

    fn extrude(&self, shape: &Shape, vector: Vector3) -> Result<Shape, GeomError> {
        if vector.length() <= 0.0 || !vector.length().is_finite() {
            return Err(OcctError::InvalidInput {
                what: "extrusion needs a non-zero vector",
            }
            .into());
        }
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let direction = ffi::gp::new_vec(vector.x, vector.y, vector.z);
        let mut make =
            ffi::b_rep_prim_api::BRepPrimAPI_MakePrism_new(inner, &direction, true, true);
        if !make.IsDone() {
            return Err(OcctError::NotDone {
                operation: "extrude",
            }
            .into());
        }
        Ok(erase(make.pin_mut().Shape()))
    }

    fn revolve(
        &self,
        shape: &Shape,
        center: Vector3,
        axis: Vector3,
        angle: f64,
    ) -> Result<Shape, GeomError> {
        if axis.length() <= 0.0 || !axis.length().is_finite() {
            return Err(OcctError::InvalidInput {
                what: "revolving needs a non-zero axis",
            }
            .into());
        }
        if !angle.is_finite() {
            return Err(OcctError::InvalidInput {
                what: "revolving needs a finite angle",
            }
            .into());
        }
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let origin = point(center);
        let direction = ffi::gp::gp_Dir_new(axis.x, axis.y, axis.z);
        let axis = ffi::gp::gp_Ax1_new(&origin, &direction);
        let mut make = ffi::b_rep_prim_api::BRepPrimAPI_MakeRevol_new(inner, &axis, angle, true);
        if !make.IsDone() {
            return Err(OcctError::NotDone {
                operation: "revolve",
            }
            .into());
        }
        Ok(erase(make.pin_mut().Shape()))
    }

    fn bounds(&self, shape: &Shape) -> Option<(Vector3, Vector3)> {
        let occt = downcast(shape).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        let mut bnd = ffi::bnd::Bnd_Box_new();
        ffi::b_rep_bnd_lib::BRepBndLib::Add(inner, bnd.pin_mut(), false);
        if bnd.IsVoid() {
            return None;
        }
        // Drop the protective gap so the box is the tight one upstream reports.
        bnd.pin_mut().SetGap(0.0);
        let (mut xmin, mut ymin, mut zmin) = (0.0, 0.0, 0.0);
        let (mut xmax, mut ymax, mut zmax) = (0.0, 0.0, 0.0);
        bnd.Get(
            &mut xmin, &mut ymin, &mut zmin, &mut xmax, &mut ymax, &mut zmax,
        );
        Some((
            Vector3::new(xmin, ymin, zmin),
            Vector3::new(xmax, ymax, zmax),
        ))
    }

    fn mass_properties(&self, shape: &Shape) -> Option<MassProperties> {
        let occt = downcast(shape).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;

        let mut linear = ffi::g_prop::GProps_new();
        ffi::b_rep_g_prop::BRepGProp::LinearProperties(inner, linear.pin_mut(), false, false);
        let length = linear.Mass();

        let mut surface = ffi::g_prop::GProps_new();
        ffi::b_rep_g_prop::BRepGProp::SurfaceProperties(inner, surface.pin_mut(), false, false);
        let area = surface.Mass();

        let mut volume = ffi::g_prop::GProps_new();
        ffi::b_rep_g_prop::BRepGProp::VolumeProperties(
            inner,
            volume.pin_mut(),
            false,
            false,
            false,
        );
        let centre = ffi::g_prop::GProp_GProps_CentreOfMass(&volume);
        Some(MassProperties {
            length,
            area,
            volume: volume.Mass(),
            center_of_mass: Vector3::new(centre.X(), centre.Y(), centre.Z()),
        })
    }

    fn center_of_mass(&self, shape: &Shape) -> Option<Vector3> {
        let occt = downcast(shape).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        // Upstream's `getShapeProperties` rule: the property regime follows what
        // the shape contains, so a compound of faces measures its surface instead
        // of a zero-mass volume (whose centre is the origin).
        let (mut x, mut y, mut z) = (0.0, 0.0, 0.0);
        if !bridge::fc_center_of_mass(inner, &mut x, &mut y, &mut z) {
            return None;
        }
        Some(Vector3::new(x, y, z))
    }

    fn is_valid(&self, shape: &Shape) -> bool {
        let Ok(occt) = downcast(shape) else {
            return false;
        };
        let guard = occt.borrow();
        match guard.as_ref() {
            Some(inner) => bridge::fc_is_valid(inner),
            None => false,
        }
    }

    fn inertia_matrix(&self, shape: &Shape) -> Option<Matrix4> {
        let occt = downcast(shape).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        let mut values = Vec::with_capacity(9);
        bridge::fc_inertia_matrix(inner, &mut values);
        if values.len() < 9 {
            return None;
        }
        let mut matrix = Matrix4::identity();
        for row in 0..3 {
            for column in 0..3 {
                matrix.m[row * 4 + column] = values[row * 3 + column];
            }
        }
        Some(matrix)
    }

    fn principal_properties(&self, shape: &Shape) -> Option<PrincipalProperties> {
        let occt = downcast(shape).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        let mut values = Vec::with_capacity(17);
        bridge::fc_principal_properties(inner, &mut values);
        if values.len() < 17 {
            return None;
        }
        let axis =
            |offset: usize| Vector3::new(values[offset], values[offset + 1], values[offset + 2]);
        Some(PrincipalProperties {
            symmetry_axis: values[0] != 0.0,
            symmetry_point: values[1] != 0.0,
            moments: (values[2], values[3], values[4]),
            axes: (axis(5), axis(8), axis(11)),
            radius_of_gyration: (values[14], values[15], values[16]),
        })
    }

    fn clean(&self, shape: &Shape) -> Option<Shape> {
        let occt = downcast(shape).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        let mut op = ffi::shape_upgrade::UnifySameDomain_new(inner, true, true, false);
        op.pin_mut().build();
        Some(erase(op.shape()))
    }

    fn project(&self, shape: &Shape, tools: &[Shape]) -> Result<Shape, GeomError> {
        // Upstream's `Shape.project` is a *normal* projection
        // (`BRepAlgo_NormalProjection`), not a parallel one: it lays each tool on
        // the target along the target's own normals, which is what wrapping a
        // sketch onto a curved face needs.
        if tools.is_empty() {
            return Err(OcctError::InvalidInput {
                what: "project needs at least one tool shape",
            }
            .into());
        }
        let target = downcast(shape)?;
        let guard = target.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let mut list = ffi::top_tools::new_list_of_shape();
        for tool in tools {
            let occt = downcast(tool)?;
            let tool_guard = occt.borrow();
            let piece = tool_guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null shape" })?;
            list.pin_mut().Append(piece);
        }
        let projected = bridge::fc_project_normal(inner, &list);
        match projected.as_ref() {
            Some(projected) => Ok(erase(projected)),
            None => Err(OcctError::NotDone {
                operation: "project",
            }
            .into()),
        }
    }

    fn make_offset(&self, shape: &Shape, distance: f64, join: i32) -> Result<Shape, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let offset = bridge::fc_offset(inner, distance, join);
        let Some(offset) = offset.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "make_offset",
            }
            .into());
        };
        Ok(erase(offset))
    }

    fn make_thickness(
        &self,
        shape: &Shape,
        closing_faces: &[Shape],
        thickness: f64,
        tolerance: f64,
    ) -> Result<Shape, GeomError> {
        if !thickness.is_finite() || thickness == 0.0 {
            return Err(OcctError::InvalidInput {
                what: "make_thickness needs a non-zero thickness",
            }
            .into());
        }
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let mut list = ffi::top_tools::new_list_of_shape();
        for face in closing_faces {
            let face_occt = downcast(face)?;
            let face_guard = face_occt.borrow();
            let face_inner = face_guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null face" })?;
            let _ = list.pin_mut().Append(face_inner);
        }
        let thickened = bridge::fc_thick_solid(inner, &list, thickness, tolerance);
        let Some(thickened) = thickened.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "make_thickness",
            }
            .into());
        };
        Ok(erase(thickened))
    }

    fn section(&self, a: &Shape, b: &Shape) -> Result<Shape, GeomError> {
        let a_occt = downcast(a)?;
        let b_occt = downcast(b)?;
        let a_guard = a_occt.borrow();
        let b_guard = b_occt.borrow();
        let sa = a_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let sb = b_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let section = bridge::fc_section(sa, sb);
        let Some(section) = section.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "section",
            }
            .into());
        };
        Ok(erase(section))
    }

    fn slice(
        &self,
        shape: &Shape,
        direction: Vector3,
        distance: f64,
    ) -> Result<Vec<Shape>, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let sliced = bridge::fc_slice(inner, direction.x, direction.y, direction.z, distance);
        let Some(sliced) = sliced.as_ref() else {
            return Err(OcctError::NotDone { operation: "slice" }.into());
        };
        let wires = sub_shape_map(sliced, TopAbs_ShapeEnum::TopAbs_WIRE);
        Ok((1..=wires.Extent())
            .map(|index| erase(wires.FindKey(index)))
            .collect())
    }

    fn general_fuse(&self, shapes: &[Shape]) -> Result<(Shape, Vec<Vec<Shape>>), GeomError> {
        if shapes.is_empty() {
            return Err(OcctError::InvalidInput {
                what: "general fuse needs at least one shape",
            }
            .into());
        }
        let mut list = ffi::top_tools::new_list_of_shape();
        for shape in shapes {
            let occt = downcast(shape)?;
            let guard = occt.borrow();
            let inner = guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null shape" })?;
            let _ = list.pin_mut().Append(inner);
        }
        let fused = bridge::fc_general_fuse(&list);
        let Some(fused) = fused.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "general_fuse",
            }
            .into());
        };
        let mut split = Vec::with_capacity(shapes.len());
        for index in 1..=shapes.len() as i32 {
            let modified = bridge::fc_general_fuse_modified(&list, index, 0.0);
            split.push(
                modified
                    .as_ref()
                    .map(|shape| top_level_parts(shape))
                    .unwrap_or_default(),
            );
        }
        Ok((erase(fused), split))
    }

    fn split_shape(
        &self,
        shape: &Shape,
        tools: &[(Shape, Shape)],
        check_interior: bool,
    ) -> Result<(Vec<Shape>, Vec<Shape>), GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;

        let mut cutters = ffi::top_tools::new_list_of_shape();
        let mut faces = ffi::top_tools::new_list_of_shape();
        for (cutter, face) in tools {
            let cutter_occt = downcast(cutter)?;
            let cutter_guard = cutter_occt.borrow();
            let cutter_inner = cutter_guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null tool" })?;
            let face_occt = downcast(face)?;
            let face_guard = face_occt.borrow();
            let face_inner = face_guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null face" })?;
            let _ = cutters.pin_mut().Append(cutter_inner);
            let _ = faces.pin_mut().Append(face_inner);
        }

        let mut direct = ffi::top_tools::new_list_of_shape();
        let mut left = ffi::top_tools::new_list_of_shape();
        if !bridge::fc_split_shape(
            inner,
            &cutters,
            &faces,
            check_interior,
            direct.pin_mut(),
            left.pin_mut(),
        ) {
            return Err(OcctError::NotDone {
                operation: "split_shape",
            }
            .into());
        }
        Ok((collect_list(&direct), collect_list(&left)))
    }

    fn optimal_bounds(
        &self,
        shape: &Shape,
        use_triangulation: bool,
        use_tolerance: bool,
    ) -> Option<(Vector3, Vector3)> {
        let occt = downcast(shape).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        let mut values = Vec::with_capacity(6);
        if !bridge::fc_optimal_box(inner, use_triangulation, use_tolerance, &mut values) {
            return None;
        }
        if values.len() < 6 {
            return None;
        }
        Some((
            Vector3::new(values[0], values[1], values[2]),
            Vector3::new(values[3], values[4], values[5]),
        ))
    }

    fn same_shape(&self, a: &Shape, b: &Shape) -> bool {
        let (Ok(a_occt), Ok(b_occt)) = (downcast(a), downcast(b)) else {
            return a.is_same(b);
        };
        let a_guard = a_occt.borrow();
        let b_guard = b_occt.borrow();
        match (a_guard.as_ref(), b_guard.as_ref()) {
            (Some(a_inner), Some(b_inner)) => bridge::fc_is_same(a_inner, b_inner),
            _ => a.is_same(b),
        }
    }

    fn count_sub_shapes(&self, shape: &Shape, kind: &str) -> Result<usize, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        if inner.IsNull() {
            return Ok(0);
        }
        // Upstream's `countSubShapes("SubShape")` counts the direct children, not
        // every descendant; every other spelling counts that sub-shape kind.
        if kind == "SubShape" {
            return Ok(bridge::fc_direct_child_count(inner).max(0) as usize);
        }
        let Some(occt_kind) = count_kind(kind) else {
            return Ok(0);
        };
        Ok(sub_shape_map(inner, occt_kind).Extent() as usize)
    }

    fn is_degenerate(&self, edge: &Shape) -> Result<bool, GeomError> {
        let occt = downcast(edge)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null edge" })?;
        if !bridge::fc_is_edge(inner) {
            return Err(OcctError::InvalidInput {
                what: "not an edge",
            }
            .into());
        }
        Ok(bridge::fc_edge_degenerated(inner))
    }

    fn is_seam(&self, edge: &Shape, face: &Shape) -> Result<bool, GeomError> {
        let edge_occt = downcast(edge)?;
        let face_occt = downcast(face)?;
        let edge_guard = edge_occt.borrow();
        let face_guard = face_occt.borrow();
        let edge_inner = edge_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null edge" })?;
        let face_inner = face_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null face" })?;
        if !bridge::fc_is_edge(edge_inner) {
            return Err(OcctError::InvalidInput {
                what: "not an edge",
            }
            .into());
        }
        if !bridge::fc_is_face(face_inner) {
            return Err(OcctError::InvalidInput { what: "not a face" }.into());
        }
        Ok(bridge::fc_edge_is_seam(edge_inner, face_inner))
    }

    fn edge_parameter_range(&self, edge: &Shape) -> Result<(f64, f64), GeomError> {
        let occt = downcast(edge)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null edge" })?;
        if !bridge::fc_is_edge(inner) {
            return Err(OcctError::InvalidInput {
                what: "not an edge",
            }
            .into());
        }
        let (mut first, mut last) = (0.0, 0.0);
        bridge::fc_edge_range(inner, &mut first, &mut last);
        Ok((first, last))
    }

    fn face_parameter_range(&self, face: &Shape) -> Result<(f64, f64, f64, f64), GeomError> {
        let occt = downcast(face)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null face" })?;
        if !bridge::fc_is_face(inner) {
            return Err(OcctError::InvalidInput { what: "not a face" }.into());
        }
        let (mut u0, mut u1, mut v0, mut v1) = (0.0, 0.0, 0.0, 0.0);
        bridge::fc_face_uv_bounds(inner, &mut u0, &mut u1, &mut v0, &mut v1);
        Ok((u0, u1, v0, v1))
    }

    fn dist_to_shape(&self, first: &Shape, second: &Shape) -> Result<ShapeDistance, GeomError> {
        let first_occt = downcast(first)?;
        let second_occt = downcast(second)?;
        let first_guard = first_occt.borrow();
        let second_guard = second_occt.borrow();
        let first_inner = first_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let second_inner = second_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let (mut distance, mut first_x, mut first_y, mut first_z) = (0.0, 0.0, 0.0, 0.0);
        let (mut second_x, mut second_y, mut second_z) = (0.0, 0.0, 0.0);
        if !bridge::fc_dist_to_shape(
            first_inner,
            second_inner,
            &mut distance,
            &mut first_x,
            &mut first_y,
            &mut first_z,
            &mut second_x,
            &mut second_y,
            &mut second_z,
        ) {
            return Err(OcctError::NotDone {
                operation: "dist_to_shape",
            }
            .into());
        }
        Ok(ShapeDistance {
            distance,
            first: Vector3::new(first_x, first_y, first_z),
            second: Vector3::new(second_x, second_y, second_z),
        })
    }

    fn is_closed(&self, shape: &Shape) -> Result<bool, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        Ok(bridge::fc_is_closed(inner))
    }

    fn shape_tolerance(&self, shape: &Shape) -> Result<f64, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        Ok(bridge::fc_shape_tolerance(inner))
    }

    fn is_inside(
        &self,
        shape: &Shape,
        point: Vector3,
        tolerance: f64,
        check_face: bool,
    ) -> Result<bool, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        match bridge::fc_is_inside(
            inner,
            point.x,
            point.y,
            point.z,
            tolerance,
            check_face,
        ) {
            1 => Ok(true),
            0 => Ok(false),
            _ => Err(OcctError::NotDone {
                operation: "is_inside",
            }
            .into()),
        }
    }

    fn is_partner(&self, first: &Shape, second: &Shape) -> Result<bool, GeomError> {
        let first_occt = downcast(first)?;
        let second_occt = downcast(second)?;
        let first_guard = first_occt.borrow();
        let second_guard = second_occt.borrow();
        match (first_guard.as_ref(), second_guard.as_ref()) {
            (Some(a), Some(b)) => Ok(bridge::fc_is_partner(a, b)),
            _ => Ok(false),
        }
    }

    fn complement(&self, shape: &Shape) -> Result<Shape, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let complemented = bridge::fc_complement(inner);
        let Some(complemented) = complemented.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "complement",
            }
            .into());
        };
        Ok(erase(complemented))
    }

    fn transform_geometry(
        &self,
        shape: &Shape,
        matrix: &Matrix4,
        copy: bool,
    ) -> Result<Shape, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let m = &matrix.m;
        if m[12].abs() > 1e-12
            || m[13].abs() > 1e-12
            || m[14].abs() > 1e-12
            || (m[15] - 1.0).abs() > 1e-12
        {
            return Err(OcctError::InvalidInput {
                what: "matrix is not affine",
            }
            .into());
        }
        let result = bridge::fc_transform_geometry(inner, m, copy);
        let Some(result) = result.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "transform_geometry",
            }
            .into());
        };
        Ok(erase(result))
    }

    fn global_tolerance(&self, shape: &Shape, mode: i32) -> Result<f64, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let value = bridge::fc_global_tolerance(inner, mode);
        if !value.is_finite() {
            return Err(OcctError::NotDone {
                operation: "global_tolerance",
            }
            .into());
        }
        Ok(value)
    }

    fn fix_tolerance(
        &self,
        shape: &Shape,
        value: f64,
        kind: ToleranceKind,
    ) -> Result<(), GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        if !bridge::fc_fix_tolerance(inner, value, tolerance_shape_type(kind)) {
            return Err(OcctError::InvalidInput {
                what: "a tolerance is a finite, non-negative number",
            }
            .into());
        }
        Ok(())
    }

    fn limit_tolerance(
        &self,
        shape: &Shape,
        minimum: f64,
        maximum: f64,
        kind: ToleranceKind,
    ) -> Result<bool, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        if !minimum.is_finite() || !maximum.is_finite() {
            return Err(OcctError::InvalidInput {
                what: "tolerances are finite numbers",
            }
            .into());
        }
        Ok(bridge::fc_limit_tolerance(
            inner,
            minimum,
            maximum,
            tolerance_shape_type(kind),
        ))
    }

    fn tolerance_in(
        &self,
        shape: &Shape,
        minimum: f64,
        maximum: f64,
        kind: ToleranceKind,
    ) -> Result<Vec<Shape>, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let mut list = ffi::top_tools::new_list_of_shape();
        bridge::fc_tolerance_in(
            inner,
            minimum,
            maximum,
            tolerance_shape_type(kind),
            list.pin_mut(),
        );
        Ok(list_shapes(&list))
    }

    fn tolerance_over(
        &self,
        shape: &Shape,
        value: f64,
        kind: ToleranceKind,
    ) -> Result<Vec<Shape>, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let mut list = ffi::top_tools::new_list_of_shape();
        bridge::fc_tolerance_over(inner, value, tolerance_shape_type(kind), list.pin_mut());
        Ok(list_shapes(&list))
    }

    fn make_wires(
        &self,
        shape: &Shape,
        _op: &str,
        tolerance: f64,
    ) -> Result<Shape, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let wires = bridge::fc_make_wires(inner, tolerance);
        let Some(wires) = wires.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "make_wires",
            }
            .into());
        };
        Ok(erase(wires))
    }

    fn remove_internal_wires(
        &self,
        shape: &Shape,
        minimum_area: f64,
    ) -> Result<(Shape, bool), GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let mut fix = bridge::fc_remove_internal_wires_new();
        let performed = fix.pin_mut().perform(inner, minimum_area);
        let result = fix.result();
        let Some(result) = result.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "remove_internal_wires",
            }
            .into());
        };
        Ok((erase(result), performed))
    }

    fn stl_bytes(&self, shape: &Shape, deflection: f64) -> Result<Vec<u8>, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        if !deflection.is_finite() || deflection <= 0.0 {
            return Err(OcctError::InvalidInput {
                what: "the deflection is a positive number",
            }
            .into());
        }
        let bytes = bridge::fc_stl_bytes(inner, deflection);
        if bytes.is_empty() {
            return Err(OcctError::NotDone {
                operation: "stl_bytes",
            }
            .into());
        }
        Ok(bytes.to_vec())
    }

    fn edge_value_at(&self, edge: &Shape, t: f64) -> Result<Vector3, GeomError> {
        let occt = downcast(edge)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null edge" })?;
        if !bridge::fc_is_edge(inner) {
            return Err(OcctError::InvalidInput {
                what: "not an edge",
            }
            .into());
        }
        let (mut x, mut y, mut z) = (0.0, 0.0, 0.0);
        if !bridge::fc_edge_value(inner, t, &mut x, &mut y, &mut z) {
            return Err(OcctError::NotDone {
                operation: "edge_value_at",
            }
            .into());
        }
        Ok(Vector3::new(x, y, z))
    }

    fn face_value_at(&self, face: &Shape, u: f64, v: f64) -> Result<Vector3, GeomError> {
        let occt = downcast(face)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null face" })?;
        if !bridge::fc_is_face(inner) {
            return Err(OcctError::InvalidInput { what: "not a face" }.into());
        }
        let (mut x, mut y, mut z) = (0.0, 0.0, 0.0);
        if !bridge::fc_face_value(inner, u, v, &mut x, &mut y, &mut z) {
            return Err(OcctError::NotDone {
                operation: "face_value_at",
            }
            .into());
        }
        Ok(Vector3::new(x, y, z))
    }

    fn edge_discretize(
        &self,
        edge: &Shape,
        mode: i32,
        value: f64,
        curvature: f64,
        minimum: i32,
        first: f64,
        last: f64,
    ) -> Result<Vec<Vector3>, GeomError> {
        let occt = downcast(edge)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null edge" })?;
        if !bridge::fc_is_edge(inner) {
            return Err(OcctError::InvalidInput {
                what: "not an edge",
            }
            .into());
        }
        let coordinates = bridge::fc_edge_discretize(
            inner, mode, value, curvature, minimum, first, last,
        );
        if coordinates.is_empty() || coordinates.len() % 3 != 0 {
            return Err(OcctError::NotDone {
                operation: "edge_discretize",
            }
            .into());
        }
        Ok(coordinates
            .chunks_exact(3)
            .map(|point| Vector3::new(point[0], point[1], point[2]))
            .collect())
    }

    fn edge_parameter_by_length(
        &self,
        edge: &Shape,
        length: f64,
        tolerance: f64,
    ) -> Result<f64, GeomError> {
        let occt = downcast(edge)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null edge" })?;
        if !bridge::fc_is_edge(inner) {
            return Err(OcctError::InvalidInput {
                what: "not an edge",
            }
            .into());
        }
        let parameter = bridge::fc_edge_parameter_by_length(inner, length, tolerance);
        if !parameter.is_finite() {
            return Err(OcctError::InvalidInput {
                what: "value out of range",
            }
            .into());
        }
        Ok(parameter)
    }

    fn shell_free_edges(&self, shell: &Shape) -> Result<Shape, GeomError> {
        let occt = downcast(shell)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shell" })?;
        let free = bridge::fc_shell_free_edges(inner);
        match free.as_ref() {
            Some(free) => Ok(erase(free)),
            None => Err(OcctError::NotDone {
                operation: "shell_free_edges",
            }
            .into()),
        }
    }

    fn bspline_surface_interpolate(
        &self,
        rows: &[Vec<Vector3>],
    ) -> Result<BsplineSurfaceData, GeomError> {
        let nx = rows.len();
        let ny = rows.first().map(Vec::len).unwrap_or(0);
        if nx < 2 || ny < 2 || rows.iter().any(|row| row.len() != ny) {
            return Err(OcctError::InvalidInput {
                what: "a B-spline surface needs a rectangular grid of at least two by two points",
            }
            .into());
        }
        let mut coordinates = Vec::with_capacity(nx * ny * 3);
        for row in rows {
            for point in row {
                coordinates.extend_from_slice(&[point.x, point.y, point.z]);
            }
        }
        let raw = bridge::fc_bspline_surface_interpolate(&coordinates, nx as i32, ny as i32);
        // bounds (4), nu + u knots, nv + v knots
        if raw.len() < 6 {
            return Err(OcctError::NotDone {
                operation: "bspline_surface_interpolate",
            }
            .into());
        }
        let mut cursor = 4;
        let mut sequence = || -> Option<Vec<f64>> {
            let length = *raw.get(cursor)? as usize;
            cursor += 1;
            let values = raw.get(cursor..cursor + length)?.to_vec();
            cursor += length;
            Some(values)
        };
        let u_knots = sequence().ok_or(OcctError::NotDone {
            operation: "bspline_surface_interpolate",
        })?;
        let v_knots = sequence().ok_or(OcctError::NotDone {
            operation: "bspline_surface_interpolate",
        })?;
        Ok(BsplineSurfaceData {
            u_knots,
            v_knots,
            u_bounds: (raw[0], raw[1]),
            v_bounds: (raw[2], raw[3]),
        })
    }

    fn bspline_surface_face(&self, rows: &[Vec<Vector3>]) -> Result<Shape, GeomError> {
        let nx = rows.len();
        let ny = rows.first().map(Vec::len).unwrap_or(0);
        if nx < 2 || ny < 2 || rows.iter().any(|row| row.len() != ny) {
            return Err(OcctError::InvalidInput {
                what: "a B-spline surface needs a rectangular grid of at least two by two points",
            }
            .into());
        }
        let mut coordinates = Vec::with_capacity(nx * ny * 3);
        for row in rows {
            for point in row {
                coordinates.extend_from_slice(&[point.x, point.y, point.z]);
            }
        }
        let face = bridge::fc_bspline_surface_face(&coordinates, nx as i32, ny as i32);
        let Some(face) = face.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "bspline_surface_face",
            }
            .into());
        };
        Ok(erase(face))
    }

    fn is_equal(&self, a: &Shape, b: &Shape) -> Result<bool, GeomError> {
        let a_occt = downcast(a)?;
        let b_occt = downcast(b)?;
        let a_guard = a_occt.borrow();
        let b_guard = b_occt.borrow();
        match (a_guard.as_ref(), b_guard.as_ref()) {
            (Some(a_inner), Some(b_inner)) => Ok(a_inner.IsEqual(b_inner)),
            _ => Ok(false),
        }
    }

    fn copy_shape(&self, shape: &Shape) -> Result<Shape, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        if inner.IsNull() {
            return Err(OcctError::InvalidInput { what: "null shape" }.into());
        }
        let copy = bridge::fc_copy_shape(inner);
        let Some(copy) = copy.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "copy_shape",
            }
            .into());
        };
        Ok(erase(copy))
    }

    fn shape_hash(&self, shape: &Shape) -> Result<u64, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        Ok(bridge::fc_shape_hash(inner, u64::MAX))
    }

    fn orientation(&self, shape: &Shape) -> Option<&'static str> {
        let occt = downcast(shape).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        Some(match inner.Orientation() {
            TopAbs_Orientation::TopAbs_REVERSED => "Reversed",
            TopAbs_Orientation::TopAbs_INTERNAL => "Internal",
            TopAbs_Orientation::TopAbs_EXTERNAL => "External",
            TopAbs_Orientation::TopAbs_FORWARD => "Forward",
            _ => "Forward",
        })
    }

    fn reversed(&self, shape: &Shape) -> Result<Shape, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let reversed = bridge::fc_reversed(inner);
        let Some(reversed) = reversed.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "reversed",
            }
            .into());
        };
        Ok(erase(reversed))
    }

    fn with_orientation(&self, shape: &Shape, orientation: &str) -> Result<Shape, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let value = match orientation {
            "Forward" => 0,
            "Reversed" => 1,
            "Internal" => 2,
            "External" => 3,
            _ => {
                return Err(OcctError::InvalidInput {
                    what: "an orientation is Forward, Reversed, Internal or External",
                }
                .into());
            }
        };
        let oriented = bridge::fc_with_orientation(inner, value);
        let Some(oriented) = oriented.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "with_orientation",
            }
            .into());
        };
        Ok(erase(oriented))
    }

    fn remove_shapes(&self, shape: &Shape, shapes: &[Shape]) -> Result<Shape, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let mut list = ffi::top_tools::new_list_of_shape();
        for removal in shapes {
            let removal_occt = downcast(removal)?;
            let removal_guard = removal_occt.borrow();
            let removal_inner = removal_guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null shape" })?;
            let _ = list.pin_mut().Append(removal_inner);
        }
        let result = bridge::fc_reshape_remove(inner, &list);
        let Some(result) = result.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "remove_shapes",
            }
            .into());
        };
        Ok(erase(result))
    }

    fn split_edge(&self, edge: &Shape, parameters: &[f64]) -> Result<Shape, GeomError> {
        let occt = downcast(edge)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null edge" })?;
        if !bridge::fc_is_edge(inner) {
            return Err(OcctError::InvalidInput {
                what: "split_edge needs an edge",
            }
            .into());
        }
        let split = bridge::fc_split_edge(inner, parameters);
        let Some(split) = split.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "split_edge",
            }
            .into());
        };
        Ok(erase(split))
    }

    fn replace_shapes(
        &self,
        shape: &Shape,
        replacements: &[(Shape, Shape)],
    ) -> Result<Shape, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let mut from = ffi::top_tools::new_list_of_shape();
        let mut to = ffi::top_tools::new_list_of_shape();
        for (old, new) in replacements {
            let old_occt = downcast(old)?;
            let old_guard = old_occt.borrow();
            let old_inner = old_guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null shape" })?;
            let new_occt = downcast(new)?;
            let new_guard = new_occt.borrow();
            let new_inner = new_guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null shape" })?;
            let _ = from.pin_mut().Append(old_inner);
            let _ = to.pin_mut().Append(new_inner);
        }
        let result = bridge::fc_reshape_replace(inner, &from, &to);
        let Some(result) = result.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "replace_shapes",
            }
            .into());
        };
        Ok(erase(result))
    }

    fn static_moments(&self, shape: &Shape) -> Option<[f64; 3]> {
        let occt = downcast(shape).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        let mut props = ffi::g_prop::GProps_new();
        ffi::b_rep_g_prop::BRepGProp::VolumeProperties(inner, props.pin_mut(), false, false, false);
        let (mut lx, mut ly, mut lz) = (0.0, 0.0, 0.0);
        props.StaticMoments(&mut lx, &mut ly, &mut lz);
        Some([lx, ly, lz])
    }

    fn place(&self, shape: &Shape, placement: &Placement) -> Result<Shape, GeomError> {
        let shape_occt = downcast(shape)?;
        let shape_guard = shape_occt.borrow();
        let s = shape_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let trsf = transform_from(placement);
        let mut op = ffi::b_rep_builder_api::BRepBuilderAPI_Transform_new(s, &trsf, true);
        let out = erase(op.pin_mut().Shape());
        if !op.IsDone() {
            return Err(OcctError::NotDone { operation: "place" }.into());
        }
        Ok(out)
    }

    fn transform(&self, shape: &Shape, matrix: &Matrix4) -> Result<Shape, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let mut trsf = ffi::gp::new_transform();
        if !bridge::fc_trsf_from_matrix(&matrix.m, trsf.pin_mut()) {
            return Err(OcctError::InvalidInput {
                what: "matrix is not an affine transform",
            }
            .into());
        }
        let mut op = ffi::b_rep_builder_api::BRepBuilderAPI_Transform_new(inner, &trsf, true);
        let out = erase(op.pin_mut().Shape());
        if !op.IsDone() {
            return Err(OcctError::NotDone {
                operation: "transform",
            }
            .into());
        }
        Ok(out)
    }

    fn general_transform(&self, shape: &Shape, matrix: &Matrix4) -> Result<Shape, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        // `gp_GTrsf` holds only the affine 3x4 block; reject a perspective
        // matrix instead of silently dropping its last row.
        let m = &matrix.m;
        if m[12].abs() > 1e-12
            || m[13].abs() > 1e-12
            || m[14].abs() > 1e-12
            || (m[15] - 1.0).abs() > 1e-12
        {
            return Err(OcctError::InvalidInput {
                what: "matrix is not affine",
            }
            .into());
        }
        let result = bridge::fc_gtransform(inner, m);
        let Some(result) = result.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "general_transform",
            }
            .into());
        };
        Ok(erase(result))
    }

    fn resolve(&self, shape: &Shape, name: &str) -> Option<Shape> {
        // Positional fallback only: `FaceN`/`EdgeN` -> the N-th sub-shape. Stable
        // *named* resolution needs the lineage mapper (a later slice); this mirrors
        // FreeCAD's behaviour when no element map is available.
        let shape_occt = downcast(shape).ok()?;
        let shape_guard = shape_occt.borrow();
        let s = shape_guard.as_ref()?;
        let (kind, index) = parse_sub_name(name)?;
        let map = sub_shape_map(s, kind);
        if index >= 1 && index <= map.Extent() {
            Some(erase(map.FindKey(index)))
        } else {
            None
        }
    }

    fn save_shape(&self, shape: &Shape) -> Vec<u8> {
        crate::shape::write_brep(shape).unwrap_or_default()
    }

    fn load_shape(&self, bytes: &[u8]) -> Option<Shape> {
        crate::shape::read_brep(bytes)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::BOOLEANS
            | Capabilities::FILLET
            | Capabilities::CHAMFER
            | Capabilities::TESSELLATION
            | Capabilities::TOPOLOGY
    }

    fn tessellate(
        &self,
        shape: &Shape,
        options: &TessellationOptions,
    ) -> Result<Tessellation, GeomError> {
        topology::tessellate(shape, options)
    }

    fn elements(&self, shape: &Shape, kind: ElementKind) -> Vec<ElementRef> {
        topology::elements(shape, kind)
    }

    fn ancestors(&self, shape: &Shape, element: &ElementRef, kind: ElementKind) -> Vec<ElementRef> {
        topology::ancestors(shape, element, kind)
    }

    fn continuity(&self, shape: &Shape, a: &ElementRef, b: &ElementRef) -> Option<Continuity> {
        topology::continuity(shape, a, b)
    }

    fn chamfer(
        &self,
        shape: &Shape,
        edges: &[ElementRef],
        d1: f64,
        d2: Option<f64>,
    ) -> Result<OpResult, GeomError> {
        topology::chamfer(shape, edges, d1, d2)
    }
}

/// Which operation a reusable upstream-style operation object is configured for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CutOperation {
    Cut,
    Fuse,
}

/// The `TopAbs_ShapeEnum` value upstream's `ShapeTypeFromPyType` picks for a
/// `Part.Shape` subclass, as the integer the sibling bridge takes.
///
/// cxx exposes a shared enum as a transparent struct, so its `repr` field is the
/// OCCT discriminant (`TopAbs_SHAPE` is 8, `TopAbs_VERTEX` 7, …).
fn tolerance_shape_type(kind: ToleranceKind) -> i32 {
    let value = match kind {
        ToleranceKind::All => TopAbs_ShapeEnum::TopAbs_SHAPE,
        ToleranceKind::Solid => TopAbs_ShapeEnum::TopAbs_SOLID,
        ToleranceKind::Shell => TopAbs_ShapeEnum::TopAbs_SHELL,
        ToleranceKind::Face => TopAbs_ShapeEnum::TopAbs_FACE,
        ToleranceKind::Wire => TopAbs_ShapeEnum::TopAbs_WIRE,
        ToleranceKind::Edge => TopAbs_ShapeEnum::TopAbs_EDGE,
        ToleranceKind::Vertex => TopAbs_ShapeEnum::TopAbs_VERTEX,
    };
    value.repr as i32
}

/// A copy of every shape in a bridged `TopTools_ListOfShape`.
fn list_shapes(list: &TopTools_ListOfShape) -> Vec<Shape> {
    let count = bridge::fc_list_size(list);
    (1..=count)
        .map(|index| erase(bridge::fc_list_value(list, index)))
        .collect()
}

/// A copy of an OCCT shape handle (reference-counted).
fn owned(shape: &TopoDS_Shape) -> UniquePtr<TopoDS_Shape> {
    ffi::topo_ds::TopoDS_Shape_to_owned(shape)
}

/// Borrow every non-null handle in `shapes`.
fn refs(shapes: &[UniquePtr<TopoDS_Shape>]) -> Vec<&TopoDS_Shape> {
    shapes.iter().filter_map(|shape| shape.as_ref()).collect()
}

/// The *direct* children of a shape, in `TopoDS_Iterator` order.
///
/// Unlike `TopExp::MapShapes`, this does not recurse: it is the iteration
/// upstream's `RecursiveAddTools`/`RecursiveCutCompound` perform.
fn direct_children(shape: &TopoDS_Shape) -> Vec<UniquePtr<TopoDS_Shape>> {
    let count = bridge::fc_shape_child_count(shape);
    (1..=count)
        .map(|index| bridge::fc_shape_child(shape, index))
        .filter(|child| !child.is_null())
        .collect()
}

/// Flatten a shape's compound children recursively, as `RecursiveAddTools` does.
fn flatten_compound(shape: &TopoDS_Shape) -> Vec<UniquePtr<TopoDS_Shape>> {
    let mut out = Vec::new();
    flatten_compound_into(shape, &mut out);
    out
}

fn flatten_compound_into(shape: &TopoDS_Shape, out: &mut Vec<UniquePtr<TopoDS_Shape>>) {
    if shape.ShapeType() == TopAbs_ShapeEnum::TopAbs_COMPOUND {
        for child in direct_children(shape) {
            if let Some(child) = child.as_ref() {
                flatten_compound_into(child, out);
            }
        }
    } else {
        out.push(owned(shape));
    }
}

/// Flatten an elementary surface into the layout `fc_make_face_on_surface`
/// takes: kind, origin, axis (the normal for a plane), the `u` direction, then
/// radius, semi-angle, major and minor radius.
pub fn surface_spec(surface: &SurfaceSpec) -> Vec<f64> {
    fn put(spec: &mut [f64], at: usize, point: Vector3) {
        spec[at] = point.x;
        spec[at + 1] = point.y;
        spec[at + 2] = point.z;
    }
    let mut spec = vec![0.0; 15];
    match surface {
        SurfaceSpec::Plane {
            origin,
            normal,
            x_dir,
            direct,
        } => {
            spec[0] = 0.0;
            put(&mut spec, 1, *origin);
            put(&mut spec, 4, *normal);
            put(&mut spec, 7, *x_dir);
            spec[14] = f64::from(u8::from(*direct));
        }
        SurfaceSpec::Cylinder {
            origin,
            axis,
            x_dir,
            radius,
            direct,
        } => {
            spec[0] = 1.0;
            put(&mut spec, 1, *origin);
            put(&mut spec, 4, *axis);
            put(&mut spec, 7, *x_dir);
            spec[10] = *radius;
            spec[14] = f64::from(u8::from(*direct));
        }
        SurfaceSpec::Cone {
            origin,
            axis,
            x_dir,
            radius,
            semi_angle,
            direct,
        } => {
            spec[0] = 2.0;
            put(&mut spec, 1, *origin);
            put(&mut spec, 4, *axis);
            put(&mut spec, 7, *x_dir);
            spec[10] = *radius;
            spec[11] = *semi_angle;
            spec[14] = f64::from(u8::from(*direct));
        }
        SurfaceSpec::Sphere {
            center,
            axis,
            x_dir,
            radius,
            direct,
        } => {
            spec[0] = 3.0;
            put(&mut spec, 1, *center);
            put(&mut spec, 4, *axis);
            put(&mut spec, 7, *x_dir);
            spec[10] = *radius;
            spec[14] = f64::from(u8::from(*direct));
        }
        SurfaceSpec::Torus {
            center,
            axis,
            x_dir,
            major_radius,
            minor_radius,
            direct,
        } => {
            spec[0] = 4.0;
            put(&mut spec, 1, *center);
            put(&mut spec, 4, *axis);
            put(&mut spec, 7, *x_dir);
            spec[12] = *major_radius;
            spec[13] = *minor_radius;
            spec[14] = f64::from(u8::from(*direct));
        }
    }
    spec
}

/// An OCCT shape list holding the given handles, in order.
fn shape_list(shapes: &[&TopoDS_Shape]) -> UniquePtr<TopTools_ListOfShape> {
    let mut list = ffi::top_tools::new_list_of_shape();
    for shape in shapes {
        let _ = list.pin_mut().Append(shape);
    }
    list
}

/// A compound of the given shapes, in order.
fn compound_of(shapes: &[&TopoDS_Shape]) -> UniquePtr<TopoDS_Shape> {
    let mut compound = ffi::topo_ds::TopoDS_Compound_new();
    let builder = ffi::b_rep::BRep_Builder_new();
    let builder = ffi::b_rep::BRep_Builder_upcast_to_topods_builder(&builder);
    builder.MakeCompound(compound.pin_mut());
    let mut compound = ffi::topo_ds::TopoDS_Compound_as_shape(compound);
    for shape in shapes {
        builder.Add(compound.pin_mut(), shape);
    }
    compound
}

/// Upstream's `FCBRepAlgoAPI_BooleanOperation::Build`, for a cut.
///
/// - **Compound tool**, one argument (`RecursiveCutFusedTools`): flatten the
///   tool's compound children. With two or more pieces, fuse them on this same
///   operation object and use the fused result (flattened again when it is a
///   compound) as the tool list. A single-child compound is cut as-is.
/// - **Compound base**, one argument (`RecursiveCutCompound`): cut each direct
///   child with the unchanged tool and assemble the per-child results into a new
///   compound. The operation object keeps the history of the last child build,
///   exactly as upstream's does.
/// - Otherwise the plain two-list boolean is built.
///
/// Returns `None` when OCCT reports the build not done.
fn build_cut_policy(
    op: &mut UniquePtr<ffi::b_rep_algo_api::BRepAlgoAPI_Cut>,
    arguments: &[&TopoDS_Shape],
    tools: &[&TopoDS_Shape],
    operation: CutOperation,
) -> Option<UniquePtr<TopoDS_Shape>> {
    if operation == CutOperation::Cut
        && arguments.len() == 1
        && tools.len() == 1
        && tools[0].ShapeType() == TopAbs_ShapeEnum::TopAbs_COMPOUND
    {
        let mut pieces = flatten_compound(tools[0]);
        if pieces.len() >= 2 {
            // Fuse the tool pieces before cutting. Upstream flips
            // `myOperation` on the cut object and calls `Build()` again.
            let first = pieces.remove(0);
            let rest = refs(&pieces);
            let fuse_arguments = [first.as_ref().expect("a tool piece")];
            let argument_list = shape_list(&fuse_arguments);
            let tool_list = shape_list(&rest);
            bridge::fc_cut_set_operation(op.pin_mut(), true);
            bridge::fc_cut_configure(op.pin_mut(), &argument_list, &tool_list);
            bridge::fc_cut_build(op.pin_mut());
            bridge::fc_cut_set_operation(op.pin_mut(), false);
            if !op.IsDone() {
                return None;
            }
            let fused = owned(op.pin_mut().Shape());
            // `RecursiveAddTools(myShape)`: a fused result that is still a
            // compound did not all touch, so cut with its pieces individually.
            pieces = flatten_compound(fused.as_ref().expect("the fused tool"));
        }

        let tool_refs = refs(&pieces);
        let argument_list = shape_list(arguments);
        let tool_list = shape_list(&tool_refs);
        bridge::fc_cut_set_operation(op.pin_mut(), false);
        bridge::fc_cut_configure(op.pin_mut(), &argument_list, &tool_list);
        bridge::fc_cut_build(op.pin_mut());
        return op.IsDone().then(|| owned(op.pin_mut().Shape()));
    }

    if operation == CutOperation::Cut
        && arguments.len() == 1
        && arguments[0].ShapeType() == TopAbs_ShapeEnum::TopAbs_COMPOUND
    {
        let mut results = Vec::new();
        for child in direct_children(arguments[0]) {
            let child = child.as_ref().expect("a compound child");
            let child_arguments = [child];
            let out = build_cut_policy(op, &child_arguments, tools, CutOperation::Cut)?;
            results.push(out);
        }
        let result_refs = refs(&results);
        return Some(compound_of(&result_refs));
    }

    let argument_list = shape_list(arguments);
    let tool_list = shape_list(tools);
    bridge::fc_cut_set_operation(op.pin_mut(), operation == CutOperation::Fuse);
    bridge::fc_cut_configure(op.pin_mut(), &argument_list, &tool_list);
    bridge::fc_cut_build(op.pin_mut());
    op.IsDone().then(|| owned(op.pin_mut().Shape()))
}

/// The sub-shape kinds a boolean reports lineage for, coarsest first.
///
/// The order is fixed so a history is deterministic; it is not the order of the
/// relation lists (those stay grouped by input), only of enumeration.
const HISTORY_KINDS: [(ElementKind, TopAbs_ShapeEnum); 3] = [
    (ElementKind::Face, TopAbs_ShapeEnum::TopAbs_FACE),
    (ElementKind::Edge, TopAbs_ShapeEnum::TopAbs_EDGE),
    (ElementKind::Vertex, TopAbs_ShapeEnum::TopAbs_VERTEX),
];

/// The OCCT shape type behind a `Shape.countElement` spelling.
///
/// Upstream matches a *prefix* of the name (`boost::starts_with`), so `Faces`
/// and `Face6` both count faces. An unknown spelling counts zero.
fn count_kind(kind: &str) -> Option<TopAbs_ShapeEnum> {
    const KINDS: [(&str, TopAbs_ShapeEnum); 8] = [
        ("Vertex", TopAbs_ShapeEnum::TopAbs_VERTEX),
        ("Edge", TopAbs_ShapeEnum::TopAbs_EDGE),
        ("Face", TopAbs_ShapeEnum::TopAbs_FACE),
        ("Wire", TopAbs_ShapeEnum::TopAbs_WIRE),
        ("Shell", TopAbs_ShapeEnum::TopAbs_SHELL),
        ("Solid", TopAbs_ShapeEnum::TopAbs_SOLID),
        ("Compound", TopAbs_ShapeEnum::TopAbs_COMPOUND),
        ("CompSolid", TopAbs_ShapeEnum::TopAbs_COMPSOLID),
    ];
    KINDS
        .iter()
        .find(|(name, _)| kind.starts_with(name))
        .map(|(_, occt)| *occt)
}

/// The result's sub-shape maps, one per kind in [`HISTORY_KINDS`] order.
///
/// Built before the operation object is probed: the result shape borrows the
/// operation immutably, and `Modified`/`Generated` need it mutably, so the two
/// borrows cannot overlap.
fn sub_shape_maps(shape: &TopoDS_Shape) -> [(ElementKind, UniquePtr<Map>); 3] {
    HISTORY_KINDS.map(|(kind, occt)| (kind, sub_shape_map(shape, occt)))
}

/// Build the [`History`] of a boolean from **both** of its operands.
///
/// Every sub-shape of both operands is enumerated, for each of the three kinds the
/// kernel reports (`Face`, `Edge`, `Vertex`); `probe` fills `mods`/`gens` for one of
/// them and reports whether OCCT considers it deleted. An output is kept only if it
/// appears in the result's map of the same kind, so an element that does not survive
/// there is dropped, as before.
///
/// Each kept relation also records the input's [`ElementId`]: which operand it came
/// from, its kind, and its 1-based ordinal in that operand's map. Names stay
/// positional (`Face1`, `Edge3`, `Vertex8`), so one operand's `Face1` and another's
/// are told apart by identity rather than by spelling. An input with no surviving
/// output is reported deleted only when the kernel says so: unchanged elements
/// remain absent from all three lists.
fn build_history<F>(
    operands: [(OperandId, &TopoDS_Shape); 2],
    out_maps: &[(ElementKind, UniquePtr<Map>); 3],
    mut probe: F,
) -> History
where
    F: FnMut(&TopoDS_Shape, &mut UniquePtr<Map>, &mut UniquePtr<Map>) -> bool,
{
    let mut history = History::default();

    for (operand, in_shape) in operands {
        for (kind_index, (kind, occt_kind)) in HISTORY_KINDS.into_iter().enumerate() {
            let in_map = sub_shape_map(in_shape, occt_kind);
            let out_map = &out_maps[kind_index].1;

            for ordinal in 1..=in_map.Extent() {
                let element = in_map.FindKey(ordinal);
                let mut mods = new_map();
                let mut gens = new_map();
                let deleted = probe(element, &mut mods, &mut gens);
                let input_id = ElementId::new(operand, kind.prefix(), ordinal as usize);

                let mut mapped = false;
                for entry in 1..=mods.Extent() {
                    let out = mods.FindKey(entry);
                    let index = bridge::fc_indexed_map_find_index(out_map, out);
                    if index > 0 {
                        history.push_modified(
                            element_ref(kind, ordinal, element),
                            input_id.clone(),
                            element_ref(kind, index, out),
                        );
                        mapped = true;
                    }
                }
                for entry in 1..=gens.Extent() {
                    let out = gens.FindKey(entry);
                    let index = bridge::fc_indexed_map_find_index(out_map, out);
                    if index > 0 {
                        history.push_generated(
                            element_ref(kind, ordinal, element),
                            input_id.clone(),
                            element_ref(kind, index, out),
                        );
                        mapped = true;
                    }
                }
                if !mapped && deleted {
                    history.push_deleted(element_ref(kind, ordinal, element), input_id);
                }
            }
        }
    }

    // The identity (`IsSame`) pass: an unchanged input keeps its name in the
    // result. Upstream's `mapSubElement` runs this before the modified/generated
    // names, so `ElementMap::apply` folds it first too.
    add_identity_relations(&mut history, operands, out_maps);

    // The reverse/forward derivation passes. They name result elements from the
    // result's own incidence graph; see `add_derived_names`.
    add_derived_names(&mut history, out_maps);

    // The identity side table is canonical: sorted by operand, kind, ordinal,
    // relation and output, so the order OCCT reported relations in cannot change
    // how a history compares. The relation lists above keep reporting order.
    history.provenance.sort();
    history
}

/// The `(kind, 1-based ordinal)` behind a positional element name.
fn element_key(name: &str) -> Option<(ElementKind, i32)> {
    for kind in [ElementKind::Face, ElementKind::Edge, ElementKind::Vertex] {
        if let Some(rest) = name.strip_prefix(kind.prefix()) {
            if let Ok(ordinal) = rest.parse::<i32>() {
                return Some((kind, ordinal));
            }
        }
    }
    None
}

/// Record every input sub-element that survives unchanged as the same result
/// sub-element (`IsSame`), under the same kind.
///
/// This is upstream's `mapSubElement(shapes)` identity mapping: it names a result
/// element after the input element it literally still is, without claiming the
/// operation modified or generated it.
fn add_identity_relations(
    history: &mut History,
    operands: [(OperandId, &TopoDS_Shape); 2],
    out_maps: &[(ElementKind, UniquePtr<Map>); 3],
) {
    for (_operand, in_shape) in operands {
        for (kind_index, (kind, occt_kind)) in HISTORY_KINDS.into_iter().enumerate() {
            let in_map = sub_shape_map(in_shape, occt_kind);
            let out_map = &out_maps[kind_index].1;
            for ordinal in 1..=in_map.Extent() {
                let element = in_map.FindKey(ordinal);
                let index = bridge::fc_indexed_map_find_index(out_map, element);
                if index > 0 {
                    history.push_identity(
                        element_ref(kind, ordinal, element),
                        element_ref(kind, index, out_map.FindKey(index)),
                    );
                }
            }
        }
    }
}

/// Name result elements from the result's incidence graph, mirroring upstream's
/// derivation passes.
///
/// 1. **Reverse** (faces, then edges): a *named* upper element names the lower
///    elements it contains that are still unnamed (`Face1` names its edges, an
///    edge names its vertices).
/// 2. **Forward** (edges, then faces): a still-unnamed higher element is named
///    from its lower elements, but only when **all** of them are named. An edge
///    uses its vertices; a face uses the edges of its outer wire, as upstream
///    does.
///
/// A derivation is recorded as a naming relation, not as operand provenance.
fn add_derived_names(history: &mut History, out_maps: &[(ElementKind, UniquePtr<Map>); 3]) {
    let face_map = &out_maps[0].1;
    let edge_map = &out_maps[1].1;
    let vertex_map = &out_maps[2].1;

    let mut named: BTreeSet<(ElementKind, i32)> = BTreeSet::new();
    for (_, output) in history.modified.iter().chain(&history.generated) {
        if let Some(key) = element_key(&output.name) {
            named.insert(key);
        }
    }
    for (_, output) in &history.identity {
        if let Some(key) = element_key(&output.name) {
            named.insert(key);
        }
    }

    // Reverse pass: face -> edge, then edge -> vertex.
    for (upper_kind, upper_map, lower_kind, lower_occt) in [
        (
            ElementKind::Face,
            face_map,
            ElementKind::Edge,
            TopAbs_ShapeEnum::TopAbs_EDGE,
        ),
        (
            ElementKind::Edge,
            edge_map,
            ElementKind::Vertex,
            TopAbs_ShapeEnum::TopAbs_VERTEX,
        ),
    ] {
        let lower_map = if lower_kind == ElementKind::Edge {
            edge_map
        } else {
            vertex_map
        };
        for index in 1..=upper_map.Extent() {
            if !named.contains(&(upper_kind, index)) {
                continue;
            }
            let upper = upper_map.FindKey(index);
            let sub = sub_shape_map(upper, lower_occt);
            for sub_index in 1..=sub.Extent() {
                let lower_shape = sub.FindKey(sub_index);
                let lower_index = bridge::fc_indexed_map_find_index(lower_map, lower_shape);
                if lower_index <= 0 || named.contains(&(lower_kind, lower_index)) {
                    continue;
                }
                history.push_derived(
                    element_ref(upper_kind, index, upper),
                    element_ref(lower_kind, lower_index, lower_map.FindKey(lower_index)),
                );
                named.insert((lower_kind, lower_index));
            }
        }
    }

    // Forward pass: edges from their vertices.
    for index in 1..=edge_map.Extent() {
        if named.contains(&(ElementKind::Edge, index)) {
            continue;
        }
        let edge = edge_map.FindKey(index);
        let Some(lower) =
            complete_lower(&edge, TopAbs_ShapeEnum::TopAbs_VERTEX, vertex_map, &named)
        else {
            continue;
        };
        for ordinal in lower {
            history.push_derived(
                element_ref(ElementKind::Vertex, ordinal, vertex_map.FindKey(ordinal)),
                element_ref(ElementKind::Edge, index, edge),
            );
        }
        named.insert((ElementKind::Edge, index));
    }

    // Forward pass: faces from the edges of their outer wire.
    for index in 1..=face_map.Extent() {
        if named.contains(&(ElementKind::Face, index)) {
            continue;
        }
        let face = face_map.FindKey(index);
        let wire = ffi::b_rep_tools::outer_wire(bridge::fc_shape_as_face(face));
        let Some(wire) = wire.as_ref() else {
            continue;
        };
        let wire = ffi::topo_ds::cast_wire_to_shape(wire);
        let Some(lower) = complete_lower(wire, TopAbs_ShapeEnum::TopAbs_EDGE, edge_map, &named)
        else {
            continue;
        };
        for ordinal in lower {
            history.push_derived(
                element_ref(ElementKind::Edge, ordinal, edge_map.FindKey(ordinal)),
                element_ref(ElementKind::Face, index, face),
            );
        }
        named.insert((ElementKind::Face, index));
    }
}

/// The distinct lower-element ordinals of `shape`, or `None` unless there is at
/// least one and every one is already `named`.
fn complete_lower(
    shape: &TopoDS_Shape,
    kind: TopAbs_ShapeEnum,
    map: &UniquePtr<Map>,
    named: &BTreeSet<(ElementKind, i32)>,
) -> Option<Vec<i32>> {
    let element_kind = if kind == TopAbs_ShapeEnum::TopAbs_EDGE {
        ElementKind::Edge
    } else {
        ElementKind::Vertex
    };
    let sub = sub_shape_map(shape, kind);
    let mut ordinals: Vec<i32> = Vec::new();
    for index in 1..=sub.Extent() {
        let index = bridge::fc_indexed_map_find_index(map, sub.FindKey(index));
        if index <= 0 || !named.contains(&(element_kind, index)) {
            return None;
        }
        if !ordinals.contains(&index) {
            ordinals.push(index);
        }
    }
    (!ordinals.is_empty()).then_some(ordinals)
}

/// A positional element reference into an operand (`FaceN`, `EdgeN`, `VertexN`).
fn element_ref(kind: ElementKind, index: i32, shape: &TopoDS_Shape) -> ElementRef {
    ElementRef {
        shape: erase(shape),
        name: format!("{}{index}", kind.prefix()),
    }
}

/// The top-level parts of a compound: its solids when it has any, else its
/// faces, then edges, then wires (the split pieces General Fuse returns).
fn top_level_parts(shape: &TopoDS_Shape) -> Vec<Shape> {
    for kind in [
        TopAbs_ShapeEnum::TopAbs_SOLID,
        TopAbs_ShapeEnum::TopAbs_SHELL,
        TopAbs_ShapeEnum::TopAbs_FACE,
        TopAbs_ShapeEnum::TopAbs_EDGE,
        TopAbs_ShapeEnum::TopAbs_WIRE,
    ] {
        let map = sub_shape_map(shape, kind);
        if map.Extent() > 0 {
            return (1..=map.Extent())
                .map(|index| erase(map.FindKey(index)))
                .collect();
        }
    }
    Vec::new()
}

/// The shapes in an OCCT shape list, in the list's order.
fn collect_list(list: &UniquePtr<TopTools_ListOfShape>) -> Vec<Shape> {
    let Some(list) = list.as_ref() else {
        return Vec::new();
    };
    let count = bridge::fc_list_size(list);
    (1..=count)
        .map(|index| erase(bridge::fc_list_value(list, index)))
        .collect()
}

/// A `gp_Trsf` equivalent to a FerroCAD [`Placement`] (rotation about the origin,
/// then translation).
fn transform_from(placement: &Placement) -> UniquePtr<ffi::gp::gp_Trsf> {
    let mut trsf = ffi::gp::new_transform();
    let angle = placement.rotation.angle();
    if angle.abs() > 1e-12 {
        let axis = placement.rotation.axis();
        let dir = ffi::gp::gp_Dir_new(axis.x, axis.y, axis.z);
        let origin = ffi::gp::new_point(0.0, 0.0, 0.0);
        let ax1 = ffi::gp::gp_Ax1_new(&origin, &dir);
        trsf.pin_mut().SetRotation(&ax1, angle);
    }
    let translation = ffi::gp::new_vec(placement.base.x, placement.base.y, placement.base.z);
    trsf.pin_mut().set_translation_vec(&translation);
    trsf
}

/// Parse a positional sub-element name (`Face6`, `Edge12`).
fn parse_sub_name(name: &str) -> Option<(TopAbs_ShapeEnum, i32)> {
    for (prefix, kind) in [
        ("Solid", TopAbs_ShapeEnum::TopAbs_SOLID),
        ("Shell", TopAbs_ShapeEnum::TopAbs_SHELL),
        ("Face", TopAbs_ShapeEnum::TopAbs_FACE),
        ("Wire", TopAbs_ShapeEnum::TopAbs_WIRE),
        ("Edge", TopAbs_ShapeEnum::TopAbs_EDGE),
        ("Vertex", TopAbs_ShapeEnum::TopAbs_VERTEX),
    ] {
        if let Some(rest) = name.strip_prefix(prefix) {
            if let Ok(index) = rest.parse::<i32>() {
                return Some((kind, index));
            }
        }
    }
    None
}

/// A `gp_Pnt` from a [`Vector3`].
fn axis_from(base: Vector3, direction: Vector3) -> Result<UniquePtr<ffi::gp::gp_Ax2>, GeomError> {
    if direction.x == 0.0 && direction.y == 0.0 && direction.z == 0.0 {
        return Err(OcctError::InvalidInput {
            what: "a primitive needs a non-zero direction",
        }
        .into());
    }
    let origin = point(base);
    let direction = ffi::gp::gp_Dir_new(direction.x, direction.y, direction.z);
    Ok(ffi::gp::gp_Ax2_new(&origin, &direction))
}

fn point(vector: Vector3) -> UniquePtr<ffi::gp::gp_Pnt> {
    ffi::gp::new_point(vector.x, vector.y, vector.z)
}

/// A flat xyz list, the form the `fc_curve.hxx` B-spline helpers take.
fn flatten(points: &[Vector3]) -> Vec<f64> {
    let mut coords = Vec::with_capacity(points.len() * 3);
    for point in points {
        coords.extend_from_slice(&[point.x, point.y, point.z]);
    }
    coords
}

/// The frame of an elliptical conic, in the form `fc_conic_edge` takes.
trait ConicFrame {
    fn conic_frame(&self) -> (Vector3, Vector3, Vector3, f64, f64);
}

impl ConicFrame for Ellipse {
    fn conic_frame(&self) -> (Vector3, Vector3, Vector3, f64, f64) {
        (
            self.center(),
            self.normal(),
            self.x_axis(),
            self.major_radius(),
            self.minor_radius(),
        )
    }
}

impl ConicFrame for Hyperbola {
    fn conic_frame(&self) -> (Vector3, Vector3, Vector3, f64, f64) {
        (
            self.center(),
            self.normal(),
            self.x_axis(),
            self.major_radius(),
            self.minor_radius(),
        )
    }
}

impl ConicFrame for Parabola {
    fn conic_frame(&self) -> (Vector3, Vector3, Vector3, f64, f64) {
        (
            self.vertex(),
            self.normal(),
            self.x_axis(),
            self.focal(),
            0.0,
        )
    }
}

/// Read the flat payload `fc_curve.hxx`'s `fc_edge_curve` writes.
///
/// See that function for the layout: a 14-value header plus a kind-specific tail.
fn parse_edge_curve(data: &[f64]) -> Result<EdgeCurve, GeomError> {
    if data.len() < 14 {
        return Err(unsupported("curve_of: the bridge returned no curve data"));
    }
    let origin = Vector3::new(data[1], data[2], data[3]);
    let direction = Vector3::new(data[4], data[5], data[6]);
    let x_axis = Vector3::new(data[7], data[8], data[9]);
    let (first, last) = (data[10], data[11]);
    let (major, minor) = (data[12], data[13]);
    let tail = &data[14..];
    match data[0] as i32 {
        1 => Ok(EdgeCurve::Line(Line::new(origin, direction)?)),
        2 => {
            let circle = Circle::with_axis(origin, direction, x_axis, major)?;
            if (last - first).abs() >= std::f64::consts::TAU - 1e-9 {
                Ok(EdgeCurve::Circle(circle))
            } else {
                Ok(EdgeCurve::Arc(Arc::new(circle, first, last)?))
            }
        }
        3 => {
            let ellipse = Ellipse::with_axis(origin, direction, x_axis, major, minor)?;
            if (last - first).abs() >= std::f64::consts::TAU - 1e-9 {
                Ok(EdgeCurve::Ellipse(ellipse))
            } else {
                Ok(EdgeCurve::EllipticalArc(EllipticalArc::new(
                    ellipse, first, last,
                )?))
            }
        }
        4 => {
            let hyperbola = Hyperbola::with_axis(origin, direction, x_axis, major, minor)?;
            Ok(EdgeCurve::ArcOfHyperbola(ArcOfHyperbola::new(
                hyperbola, first, last,
            )?))
        }
        5 => {
            let parabola = Parabola::with_axis(origin, direction, x_axis, major)?;
            Ok(EdgeCurve::ArcOfParabola(ArcOfParabola::new(
                parabola, first, last,
            )?))
        }
        6 => parse_bezier(tail, first, last),
        7 => parse_bspline(tail),
        8 => parse_offset(tail, direction, major, first, last),
        _ => Err(unsupported("curve_of: unsupported curve type")),
    }
}

/// `count` consecutive xyz triples from `values`.
fn parse_points(values: &[f64], count: usize) -> Result<Vec<Vector3>, GeomError> {
    let count3 = count.checked_mul(3).ok_or_else(|| unsupported("curve_of: truncated poles"))?;
    let coordinates = values
        .get(..count3)
        .ok_or_else(|| unsupported("curve_of: truncated poles"))?;
    Ok(coordinates
        .chunks_exact(3)
        .map(|triple| Vector3::new(triple[0], triple[1], triple[2]))
        .collect())
}

/// A Bezier payload: `[pole_count] poles weights`.
fn parse_bezier(tail: &[f64], first: f64, last: f64) -> Result<EdgeCurve, GeomError> {
    let count = tail
        .first()
        .map(|value| *value as usize)
        .ok_or_else(|| unsupported("curve_of: truncated Bezier"))?;
    let poles = parse_points(&tail[1..], count)?;
    let weight_start = 1 + count * 3;
    let weights = tail
        .get(weight_start..weight_start + count)
        .ok_or_else(|| unsupported("curve_of: truncated Bezier weights"))?
        .to_vec();
    let base = BezierCurve::new(poles, weights)?;
    // `Geom_BezierCurve` is always parameterised on `[0, 1]`; a narrower edge
    // range selects the segment while keeping the edge's parameters.
    let curve = if (first - 0.0).abs() <= 1e-12 && (last - 1.0).abs() <= 1e-12 {
        base
    } else {
        base.trim(first, last).unwrap_or(base)
    };
    Ok(EdgeCurve::BezierCurve(curve))
}

/// A B-spline payload: `[degree, pole_count, knot_count, mult_count, periodic]`
/// poles weights knots mults.
fn parse_bspline(tail: &[f64]) -> Result<EdgeCurve, GeomError> {
    let header = tail
        .get(..5)
        .ok_or_else(|| unsupported("curve_of: truncated B-spline"))?;
    let degree = header[0] as i32;
    let pole_count = header[1] as usize;
    let knot_count = header[2] as usize;
    let mult_count = header[3] as usize;
    let periodic = header[4] != 0.0;
    let mut at = 5;
    let poles = parse_points(
        tail.get(at..).ok_or_else(|| unsupported("curve_of: truncated B-spline"))?,
        pole_count,
    )?;
    at += pole_count * 3;
    let weights = tail
        .get(at..at + pole_count)
        .ok_or_else(|| unsupported("curve_of: truncated B-spline weights"))?
        .to_vec();
    at += pole_count;
    let knots = tail
        .get(at..at + knot_count)
        .ok_or_else(|| unsupported("curve_of: truncated B-spline knots"))?
        .to_vec();
    at += knot_count;
    let mults = tail
        .get(at..at + mult_count)
        .ok_or_else(|| unsupported("curve_of: truncated B-spline multiplicities"))?
        .iter()
        .map(|value| *value as i32)
        .collect();
    let curve = if periodic {
        NurbsCurve::new_periodic(degree, knots, mults, poles, weights)?
    } else {
        NurbsCurve::new(degree, knots, mults, poles, weights)?
    };
    Ok(EdgeCurve::BSplineCurve(curve))
}

/// An offset payload: `[basis_length]` then a nested payload.
fn parse_offset(
    tail: &[f64],
    direction: Vector3,
    offset: f64,
    first: f64,
    last: f64,
) -> Result<EdgeCurve, GeomError> {
    let length = tail
        .first()
        .map(|value| *value as usize)
        .ok_or_else(|| unsupported("curve_of: truncated offset curve"))?;
    let basis_data = tail
        .get(1..1 + length)
        .ok_or_else(|| unsupported("curve_of: truncated offset basis"))?;
    let basis = parse_edge_curve(basis_data)?;
    Ok(EdgeCurve::OffsetCurve(OffsetCurve::new(
        basis, offset, direction, first, last,
    )?))
}

/// A trimmed conic edge (ellipse, hyperbola or parabola) from its frame.
fn edge_from_conic<T: ConicFrame>(
    kind: i32,
    curve: &T,
    first: f64,
    last: f64,
    what: &'static str,
) -> Result<Shape, GeomError> {
    let (center, normal, x_axis, major, minor) = curve.conic_frame();
    let edge = bridge::fc_conic_edge(
        kind,
        center.x,
        center.y,
        center.z,
        normal.x,
        normal.y,
        normal.z,
        x_axis.x,
        x_axis.y,
        x_axis.z,
        major,
        minor,
        first,
        last,
    );
    let Some(edge) = edge.as_ref() else {
        return Err(OcctError::NotDone { operation: what }.into());
    };
    Ok(erase(edge))
}

/// A Bezier edge (`Geom_BezierCurve`).
///
/// OCCT parameterises a Bezier on `[0, 1]`; a curve stored over another range is
/// re-expressed there (the geometry is unchanged, but the edge's parameters are
/// `[0, 1]`). The Python `toShape` path keeps arbitrary ranges by converting to a
/// B-spline instead.
fn edge_from_bezier(curve: &BezierCurve) -> Result<Shape, GeomError> {
    let edge = bridge::fc_bezier_edge(&flatten(curve.poles()), curve.weights());
    let Some(edge) = edge.as_ref() else {
        return Err(OcctError::NotDone {
            operation: "make_edge (Bezier)",
        }
        .into());
    };
    Ok(erase(edge))
}

/// A B-spline edge, re-segmenting when the stored range is narrower than the knot
/// domain.
fn edge_from_nurbs(curve: &NurbsCurve) -> Result<Shape, GeomError> {
    let edge = bridge::fc_bspline_edge_range(
        &flatten(curve.poles()),
        curve.weights(),
        curve.knots(),
        curve.mults(),
        curve.degree(),
        curve.is_periodic(),
        curve.first(),
        curve.last(),
    );
    let Some(edge) = edge.as_ref() else {
        return Err(OcctError::NotDone {
            operation: "make_edge (B-spline)",
        }
        .into());
    };
    Ok(erase(edge))
}

/// A circular edge from the analytic circle.
///
/// The edge's parameterisation origin is OCCT's choice: `gp_Ax2` is built from a
/// point and a normal only, so the curve comes back with its own `x_axis` after
/// [`curve_of`](GeometryBackend::curve_of). The geometry (centre, normal, radius)
/// is what round-trips; an arc's angles are carried by its three points instead.
fn edge_from_circle(circle: &Circle) -> Result<Shape, GeomError> {
    let center = point(circle.center());
    let normal = ffi::gp::gp_Dir_new(circle.normal().x, circle.normal().y, circle.normal().z);
    let axis = ffi::gp::gp_Ax2_new(&center, &normal);
    let circ = ffi::gp::gp_Circ_new(&axis, circle.radius());
    let mut make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeEdge_circle(&circ);
    if !make.IsDone() {
        return Err(OcctError::NotDone {
            operation: "make_edge (circle)",
        }
        .into());
    }
    Ok(erase(ffi::topo_ds::cast_edge_to_shape(
        make.pin_mut().Edge(),
    )))
}

/// A full elliptical edge from the analytic ellipse.
///
/// The frame goes through explicitly (centre, plane normal, major-axis direction,
/// the two radii) because `gp_Elips`' axis placement carries all of them; the
/// edge parameterisation starts on the major axis.
fn edge_from_ellipse(ellipse: &Ellipse) -> Result<Shape, GeomError> {
    let edge = bridge::fc_ellipse_edge(
        ellipse.center().x,
        ellipse.center().y,
        ellipse.center().z,
        ellipse.normal().x,
        ellipse.normal().y,
        ellipse.normal().z,
        ellipse.x_axis().x,
        ellipse.x_axis().y,
        ellipse.x_axis().z,
        ellipse.major_radius(),
        ellipse.minor_radius(),
    );
    let Some(edge) = edge.as_ref() else {
        return Err(OcctError::NotDone {
            operation: "make_edge (ellipse)",
        }
        .into());
    };
    Ok(erase(edge))
}

#[cfg(test)]
mod provenance_tests {
    //! Golden histories for the two-input booleans: both operands, all three
    //! element kinds, and the identity behind every reported relation.
    //!
    //! The counts are the OCCT 7.8 answer for two coincident-sized boxes, one offset
    //! by `(5, 5, 5)`. They are exact on purpose: this is the baseline S4 hands to
    //! the lineage layer, so a kernel or adapter change should be visible here first.

    use std::collections::{BTreeMap, BTreeSet};

    use ferrocad_geom::{ElementKind, GeometryBackend, History, OperandId, Relation};
    use ferrocad_types::Rotation;

    use super::*;

    /// Overlapping 10³ boxes: `a` at the origin, `b` shifted by `(5, 5, 5)`.
    fn overlapping_boxes(backend: &OcctBackend) -> (Shape, Shape) {
        let a = backend.make_box(10.0, 10.0, 10.0).unwrap();
        let b = backend
            .place(
                &backend.make_box(10.0, 10.0, 10.0).unwrap(),
                &Placement {
                    base: Vector3::new(5.0, 5.0, 5.0),
                    rotation: Rotation::identity(),
                },
            )
            .unwrap();
        (a, b)
    }

    /// `(operand, kind, relation) -> count` over the provenance side table.
    fn counts(history: &History) -> BTreeMap<(OperandId, String, Relation), usize> {
        let mut counts = BTreeMap::new();
        for entry in &history.provenance {
            *counts
                .entry((
                    entry.input.operand(),
                    entry.input.kind().to_string(),
                    entry.relation,
                ))
                .or_insert(0) += 1;
        }
        counts
    }

    fn expected(
        entries: &[(OperandId, &str, Relation, usize)],
    ) -> BTreeMap<(OperandId, String, Relation), usize> {
        entries
            .iter()
            .map(|(operand, kind, relation, count)| {
                ((*operand, kind.to_string(), *relation), *count)
            })
            .collect()
    }

    /// Split a positional name into its kind prefix and 1-based ordinal.
    fn split_name(name: &str) -> (String, usize) {
        for prefix in ["Face", "Edge", "Vertex"] {
            if let Some(rest) = name.strip_prefix(prefix) {
                let ordinal = rest.parse().expect("a positional ordinal");
                return (prefix.to_string(), ordinal);
            }
        }
        panic!("not a positional sub-element name: {name}");
    }

    /// The relation lists projected onto `(kind, ordinal, relation, output)`.
    ///
    /// The relation lists have no operand field, so this is compared with the
    /// provenance projection to check that every reported relation has exactly one
    /// identity entry; the per-operand counts above then check the operand split.
    fn relation_projection(history: &History) -> Vec<(String, usize, Relation, Option<String>)> {
        let mut entries = Vec::new();
        for (input, output) in &history.modified {
            let (kind, ordinal) = split_name(&input.name);
            entries.push((kind, ordinal, Relation::Modified, Some(output.name.clone())));
        }
        for (input, output) in &history.generated {
            let (kind, ordinal) = split_name(&input.name);
            entries.push((
                kind,
                ordinal,
                Relation::Generated,
                Some(output.name.clone()),
            ));
        }
        for input in &history.deleted {
            let (kind, ordinal) = split_name(&input.name);
            entries.push((kind, ordinal, Relation::Deleted, None));
        }
        entries.sort();
        entries
    }

    fn provenance_projection(history: &History) -> Vec<(String, usize, Relation, Option<String>)> {
        let mut entries: Vec<_> = history
            .provenance
            .iter()
            .map(|entry| {
                (
                    entry.input.kind().to_string(),
                    entry.input.ordinal(),
                    entry.relation,
                    entry.output.clone(),
                )
            })
            .collect();
        entries.sort();
        entries
    }

    /// The ordered names in each relation list, for run-to-run comparison.
    fn relation_names(
        history: &History,
    ) -> (Vec<(String, String)>, Vec<(String, String)>, Vec<String>) {
        (
            history
                .modified
                .iter()
                .map(|(input, output)| (input.name.clone(), output.name.clone()))
                .collect(),
            history
                .generated
                .iter()
                .map(|(input, output)| (input.name.clone(), output.name.clone()))
                .collect(),
            history
                .deleted
                .iter()
                .map(|input| input.name.clone())
                .collect(),
        )
    }

    /// Every identity is in range for its operand and every output resolves in the
    /// result, under the same kind. Names stay positional, so the two must agree.
    fn assert_histories_are_coherent(
        backend: &OcctBackend,
        operands: [(&Shape, OperandId); 2],
        result: &Shape,
        history: &History,
    ) {
        let kinds = [ElementKind::Face, ElementKind::Edge, ElementKind::Vertex];
        let total = |shape: &Shape, kind: ElementKind| backend.elements(shape, kind).len();

        let mut outputs = BTreeMap::new();
        for kind in kinds {
            outputs.insert(kind.prefix(), total(result, kind));
        }

        for entry in &history.provenance {
            let operand = operands
                .iter()
                .find(|(_, id)| *id == entry.input.operand())
                .map(|(shape, _)| *shape)
                .expect("a known operand");
            let kind = kinds
                .iter()
                .find(|kind| kind.prefix() == entry.input.kind())
                .copied()
                .expect("Face, Edge or Vertex");

            let input_total = total(operand, kind);
            assert!(
                (1..=input_total).contains(&entry.input.ordinal()),
                "{} {}#{} is outside the operand's {input_total} elements",
                entry.input.kind(),
                entry.input.operand().position(),
                entry.input.ordinal(),
            );

            if let Some(output) = &entry.output {
                let (output_kind, ordinal) = split_name(output);
                assert_eq!(
                    output_kind,
                    entry.input.kind(),
                    "an input keeps its kind in the result"
                );
                let output_total = outputs[output_kind.as_str()];
                assert!(
                    ordinal >= 1 && ordinal <= output_total,
                    "output {output} is outside the result's {output_total} {output_kind}s",
                );
            }
        }

        assert_eq!(
            provenance_projection(history),
            relation_projection(history),
            "every relation has exactly one identity entry",
        );

        let operand_ids: BTreeSet<_> = history
            .provenance
            .iter()
            .map(|entry| entry.input.operand())
            .collect();
        assert_eq!(
            operand_ids,
            BTreeSet::from([OperandId::A, OperandId::B]),
            "both operands are reported",
        );
    }

    /// The intrinsic map of a container: every element named once, nothing else.
    ///
    /// These are upstream's `ElementMapSize` counts for the same shapes: two boxes
    /// combine to 52 names, a general-fused pair to 60, and a circle swept along a
    /// line to 6. A primitive and a compound of lower elements keep no intrinsic
    /// map; an operation result uses its own history.
    #[test]
    fn a_container_names_its_own_elements() {
        use ferrocad_geom::{Lineage, MapVersion};
        let backend = OcctBackend::new();
        let a = backend.make_box(1.0, 2.0, 2.0).unwrap();
        let b = backend.make_box(2.0, 1.0, 2.0).unwrap();

        // A primitive has no intrinsic map; an operation result gets its names from
        // the operation (even when OCCT hands it back as a compound).
        assert!(backend.intrinsic_history(&a).is_none());
        let fused = backend.fuse(&a, &b).unwrap();
        let lineage = Lineage::from_history(&fused.history, MapVersion::default());
        assert_eq!(lineage.reverse().len(), 58, "the boolean's own history");

        // A compound of two boxes: 2 × (6 faces + 12 edges + 8 vertices).
        let compound = backend.make_compound(&[a.clone(), b.clone()]).unwrap();
        let history = backend
            .intrinsic_history(&compound)
            .expect("a compound of bodies names itself");
        assert_eq!(history.identity.len(), 52);
        assert!(history.modified.is_empty());
        assert!(history.generated.is_empty());
        assert!(history.deleted.is_empty());
        let lineage = Lineage::from_history(&history, MapVersion::default());
        assert_eq!(lineage.reverse().len(), 52);

        // A compound of lower elements is an internal container, not a body.
        let faces: Vec<Shape> = backend
            .elements(&a, ElementKind::Face)
            .into_iter()
            .map(|element| element.shape)
            .collect();
        let lower = backend.make_compound(&faces).unwrap();
        assert!(backend.intrinsic_history(&lower).is_none());

        // General fuse: the result compound names all 60 elements.
        let (result, _split) = backend.general_fuse(&[a.clone(), b.clone()]).unwrap();
        let history = backend
            .intrinsic_history(&result)
            .expect("the result names itself");
        assert_eq!(history.identity.len(), 60);

        // A sweep names its six elements (one face, three edges, two vertices).
        use ferrocad_curves::{Circle, LineSegment};
        let profile = backend
            .make_edge(&ferrocad_curves::EdgeCurve::Circle(
                Circle::new(Vector3::zero(), Vector3::new(0.0, 0.0, 1.0), 5.0).unwrap(),
            ))
            .unwrap();
        let spine = backend
            .make_edge(&ferrocad_curves::EdgeCurve::LineSegment(
                LineSegment::new(Vector3::zero(), Vector3::new(0.0, 0.0, 10.0)).unwrap(),
            ))
            .unwrap();
        let sweep = backend.make_pipe(&spine, &profile, 0.001).unwrap();
        assert_eq!(backend.name_every_element(&sweep).identity.len(), 6);
    }

    #[test]
    fn fuse_reports_both_operands_and_all_three_kinds() {
        let backend = OcctBackend::new();
        let (a, b) = overlapping_boxes(&backend);
        let out = backend.fuse(&a, &b).unwrap();

        assert_eq!(
            counts(&out.history),
            expected(&[
                (OperandId::A, "Face", Relation::Modified, 3),
                (OperandId::A, "Edge", Relation::Modified, 3),
                (OperandId::A, "Vertex", Relation::Deleted, 1),
                (OperandId::B, "Face", Relation::Modified, 3),
                (OperandId::B, "Edge", Relation::Modified, 3),
                (OperandId::B, "Vertex", Relation::Deleted, 1),
            ]),
        );
        assert!(
            out.history.generated.is_empty(),
            "fuse generates no new elements"
        );

        assert_histories_are_coherent(
            &backend,
            [(&a, OperandId::A), (&b, OperandId::B)],
            &out.shape,
            &out.history,
        );

        // Identity is canonical: the side table is sorted even though the relation
        // lists stay in reporting order.
        let mut sorted = out.history.provenance.clone();
        sorted.sort();
        assert_eq!(sorted, out.history.provenance);
    }

    #[test]
    fn cut_reports_both_operands_and_all_three_kinds() {
        let backend = OcctBackend::new();
        let (a, b) = overlapping_boxes(&backend);
        let out = backend.cut(&a, &b).unwrap();

        assert_eq!(
            counts(&out.history),
            expected(&[
                (OperandId::A, "Face", Relation::Modified, 3),
                (OperandId::A, "Edge", Relation::Modified, 3),
                (OperandId::A, "Vertex", Relation::Deleted, 1),
                // The tool contributes the new walls; the rest of it is consumed.
                (OperandId::B, "Face", Relation::Modified, 3),
                (OperandId::B, "Face", Relation::Deleted, 3),
                (OperandId::B, "Edge", Relation::Modified, 3),
                (OperandId::B, "Edge", Relation::Deleted, 9),
                (OperandId::B, "Vertex", Relation::Deleted, 7),
            ]),
        );
        assert!(
            out.history.generated.is_empty(),
            "cut generates no new elements"
        );

        assert_histories_are_coherent(
            &backend,
            [(&a, OperandId::A), (&b, OperandId::B)],
            &out.shape,
            &out.history,
        );
    }

    #[test]
    fn the_same_boolean_twice_yields_the_same_history() {
        let backend = OcctBackend::new();
        let (a, b) = overlapping_boxes(&backend);

        for (first, second) in [
            (backend.fuse(&a, &b).unwrap(), backend.fuse(&a, &b).unwrap()),
            (backend.cut(&a, &b).unwrap(), backend.cut(&a, &b).unwrap()),
        ] {
            assert_eq!(
                first.history.provenance, second.history.provenance,
                "identity is deterministic",
            );
            assert_eq!(
                relation_names(&first.history),
                relation_names(&second.history),
                "the relation lists are deterministic",
            );
        }
    }
}

#[cfg(test)]
mod curve_bridge_tests {
    //! The analytic curve helpers behind `Part.Circle`/`Part.Ellipse` and the
    //! explicit-knot `Part.BSplineCurve`: round-trip the OCCT side without
    //! involving the Python binding.

    use ferrocad_curves::{EdgeCurve, Ellipse, Vector3};
    use ferrocad_geom::GeometryBackend;

    use super::*;

    /// The layout `fc_bspline_push_state` writes.
    const DEGREE: usize = 0;
    const NB_POLES: usize = 2;
    const NB_KNOTS: usize = 3;
    const PERIODIC: usize = 4;
    const FIRST_PARAMETER: usize = 12;
    const LAST_PARAMETER: usize = 13;
    const CONTINUITY: usize = 16;

    #[test]
    fn an_ellipse_round_trips_through_an_edge() {
        let backend = OcctBackend::new();
        let ellipse = Ellipse::with_axis(
            Vector3::new(1.0, 2.0, 3.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
            5.0,
            3.0,
        )
        .unwrap();
        let edge = backend
            .make_edge(&EdgeCurve::Ellipse(ellipse))
            .expect("an ellipse has an OCCT edge");

        let EdgeCurve::Ellipse(back) = backend.curve_of(&edge).unwrap() else {
            panic!("the ellipse edge's basis curve is an ellipse");
        };
        assert!((back.center().sub(&ellipse.center())).length() < 1e-9);
        assert!((back.major_radius() - 5.0).abs() < 1e-9);
        assert!((back.minor_radius() - 3.0).abs() < 1e-9);
        assert!((back.normal().sub(&ellipse.normal())).length() < 1e-9);
        assert!((back.x_axis().sub(&ellipse.x_axis())).length() < 1e-9);
    }

    /// The upstream `PartTestBSplineCurve` NURBS: four poles and the knots
    /// `(0, 0.5, 1)` with multiplicities `(3, 1, 3)` at degree 2.
    fn upstream_nurbs_poles() -> [Vector3; 4] {
        [
            Vector3::new(0.0, 0.0, 0.0),
            Vector3::new(1.0, 1.0, 0.0),
            Vector3::new(2.0, 0.0, 0.0),
            Vector3::new(1.0, -1.0, 0.0),
        ]
    }

    #[test]
    fn an_explicit_knot_vector_reads_back_from_occt() {
        let backend = OcctBackend::new();
        let poles = upstream_nurbs_poles();
        let state = backend
            .bspline_state(
                &poles,
                &[1.0; 4],
                &[0.0, 0.5, 1.0],
                &[3, 1, 3],
                2,
                false,
                true,
            )
            .expect("the upstream NURBS builds");

        assert_eq!(state[DEGREE], 2.0);
        assert_eq!(state[NB_POLES], 4.0);
        assert_eq!(state[NB_KNOTS], 3.0);
        assert_eq!(state[PERIODIC], 0.0);
        assert_eq!(state[FIRST_PARAMETER], 0.0);
        assert_eq!(state[LAST_PARAMETER], 1.0);
        assert_eq!(
            state[CONTINUITY], 2.0,
            "C1 at the inner knot (mult 1, degree 2)"
        );

        // The same arrays also build the edge, exactly.
        let edge = backend
            .make_bspline_poles_mults_knots(
                &poles,
                &[1.0; 4],
                &[0.0, 0.5, 1.0],
                &[3, 1, 3],
                2,
                false,
                true,
            )
            .expect("the explicit B-spline has an edge");
        assert!(
            matches!(backend.curve_of(&edge), Ok(EdgeCurve::BSplineCurve(_))),
            "a B-spline now reports its poles, weights and knots"
        );
        assert_eq!(backend.elements(&edge, ElementKind::Edge).len(), 1);
    }

    #[test]
    fn the_periodic_conversion_restructures_the_curve() {
        let backend = OcctBackend::new();
        let poles = [
            Vector3::new(0.0, 0.0, 0.0),
            Vector3::new(1.0, 1.0, 0.0),
            Vector3::new(2.0, 0.0, 0.0),
        ];
        let state = backend
            .bspline_state(&poles, &[1.0; 3], &[0.0, 1.0], &[3, 3], 2, false, true)
            .expect("the clamped quadratic builds");
        assert_eq!(state[PERIODIC], 0.0);
        assert_eq!(state[NB_POLES], 3.0);

        // `SetPeriodic` is a structural change (the curve is not closed, so OCCT
        // drops the redundant pole); the state must come from OCCT, not from a
        // flag flip.
        let periodic = backend
            .bspline_periodic_state(
                &poles,
                &[1.0; 3],
                &[0.0, 1.0],
                &[3, 3],
                2,
                false,
                true,
                true,
            )
            .expect("SetPeriodic succeeds");
        assert_eq!(periodic[PERIODIC], 1.0);
        assert_eq!(periodic[NB_POLES], 2.0);
    }

    #[test]
    fn a_periodic_bspline_round_trips_through_an_edge() {
        use std::f64::consts::TAU;

        let backend = OcctBackend::new();
        // OCCT's own form for a full circle after `BRepBuilderAPI_NurbsConvert`:
        // three rational quadratic arcs, each knot doubled, period `2π`.
        let step = TAU / 3.0;
        let poles = [
            Vector3::new(5.0, 0.0, 10.0),
            Vector3::new(5.0, 8.660_254_037_844_387, 10.0),
            Vector3::new(-2.5, 4.330_127_018_922_193, 10.0),
            Vector3::new(-10.0, 0.0, 10.0),
            Vector3::new(-2.5, -4.330_127_018_922_193, 10.0),
            Vector3::new(5.0, -8.660_254_037_844_387, 10.0),
        ];
        let weights = [1.0, 0.5, 1.0, 0.5, 1.0, 0.5];
        let knots = [0.0, step, 2.0 * step, TAU];
        let mults = [2, 2, 2, 2];

        let edge = backend
            .make_bspline_poles_mults_knots(&poles, &weights, &knots, &mults, 2, true, true)
            .expect("a periodic spline has an edge");
        let EdgeCurve::BSplineCurve(curve) = backend.curve_of(&edge).unwrap() else {
            panic!("a periodic spline reports its B-spline form");
        };
        assert!(curve.is_periodic());
        assert_eq!(curve.degree(), 2);
        assert_eq!(curve.poles().len(), 6);
        assert_eq!(curve.weights(), &weights[..]);
        assert_eq!(curve.knots(), &knots[..]);
        assert_eq!(curve.mults(), &mults[..]);
        assert!((curve.first() - 0.0).abs() < 1e-12);
        assert!((curve.last() - TAU).abs() < 1e-12);

        // Parameter 0 is the joint at `+X`; the period end returns to it.
        let start = curve.point_at(0.0).unwrap();
        assert!((start.sub(&Vector3::new(5.0, 0.0, 10.0))).length() < 1e-9);
        assert!((curve.point_at(TAU).unwrap().sub(&start)).length() < 1e-9);

        // `make_edge` rebuilds the same periodic curve, not a clamped copy.
        let rebuilt = backend
            .make_edge(&EdgeCurve::BSplineCurve(curve.clone()))
            .expect("the periodic curve has an edge");
        let EdgeCurve::BSplineCurve(back) = backend.curve_of(&rebuilt).unwrap() else {
            panic!("the rebuilt edge still reports a B-spline");
        };
        assert!(back.is_periodic());
        assert_eq!(back.poles().len(), 6);
        for parameter in [0.0, 0.4, 1.7, 4.2, TAU - 0.1] {
            let point = back.point_at(parameter).unwrap();
            assert!(
                point.sub(&curve.point_at(parameter).unwrap()).length() < 1e-9,
                "the rebuilt curve moved at {parameter}"
            );
            let radius = (point.x * point.x + point.y * point.y).sqrt();
            assert!((radius - 5.0).abs() < 1e-9);
        }
    }

    #[test]
    fn the_resolution_is_reported_by_occt() {
        let backend = OcctBackend::new();
        let poles = upstream_nurbs_poles();
        let resolution = backend
            .bspline_resolution(
                &poles,
                &[1.0; 4],
                &[0.0, 0.5, 1.0],
                &[3, 1, 3],
                2,
                false,
                true,
                0.5,
            )
            .expect("the resolution is finite");
        assert!(resolution > 0.0);
    }
}

#[cfg(test)]
mod shape_query_tests {
    //! The `Shape.countElement`/`Edge.Degenerated`/`Edge.isSeam`/
    //! `ParameterRange`/`Shape.isEqual` backend queries, against real OCCT
    //! primitives.

    use std::f64::consts::TAU;

    use ferrocad_geom::{ElementKind, GeometryBackend};

    use super::*;

    fn box_shape() -> Shape {
        OcctBackend::new().make_box(2.0, 3.0, 4.0).expect("a box")
    }

    #[test]
    fn count_element_counts_a_boxes_topology() {
        let backend = OcctBackend::new();
        let shape = box_shape();

        assert_eq!(backend.count_sub_shapes(&shape, "Face").unwrap(), 6);
        assert_eq!(backend.count_sub_shapes(&shape, "Edge").unwrap(), 12);
        assert_eq!(backend.count_sub_shapes(&shape, "Vertex").unwrap(), 8);
        assert_eq!(backend.count_sub_shapes(&shape, "Solid").unwrap(), 1);
        assert_eq!(backend.count_sub_shapes(&shape, "Shell").unwrap(), 1);
        assert_eq!(backend.count_sub_shapes(&shape, "Compound").unwrap(), 0);
        assert_eq!(backend.count_sub_shapes(&shape, "CompSolid").unwrap(), 0);

        // Upstream matches a prefix of the spelling.
        assert_eq!(backend.count_sub_shapes(&shape, "Faces").unwrap(), 6);
        // `SubShape` is the direct children, here the one solid.
        assert_eq!(backend.count_sub_shapes(&shape, "SubShape").unwrap(), 1);
        // An unknown spelling counts zero, as upstream's silent type lookup.
        assert_eq!(backend.count_sub_shapes(&shape, "Bogus").unwrap(), 0);
    }

    #[test]
    fn a_cone_apex_has_a_degenerate_edge() {
        let backend = OcctBackend::new();
        let cone = backend
            .make_cone(5.0, 0.0, 10.0, Vector3::zero(), Vector3::new(0.0, 0.0, 1.0))
            .expect("a cone with an apex");

        let edges = backend.elements(&cone, ElementKind::Edge);
        assert!(
            edges
                .iter()
                .any(|edge| backend.is_degenerate(&edge.shape).unwrap()),
            "the apex edge collapses to a point"
        );

        let box_shape = box_shape();
        assert!(
            backend
                .elements(&box_shape, ElementKind::Edge)
                .iter()
                .all(|edge| !backend.is_degenerate(&edge.shape).unwrap()),
            "a box has no degenerate edge"
        );
    }

    #[test]
    fn a_cylinder_seam_is_closed_on_its_face() {
        let backend = OcctBackend::new();
        let cylinder = backend
            .make_cylinder(5.0, 10.0, Vector3::zero(), Vector3::new(0.0, 0.0, 1.0))
            .expect("a cylinder");
        let edges = backend.elements(&cylinder, ElementKind::Edge);
        let faces = backend.elements(&cylinder, ElementKind::Face);

        assert!(
            edges.iter().any(|edge| faces
                .iter()
                .any(|face| backend.is_seam(&edge.shape, &face.shape).unwrap())),
            "a full cylinder has a seam edge"
        );

        let box_shape = box_shape();
        let box_edges = backend.elements(&box_shape, ElementKind::Edge);
        let box_faces = backend.elements(&box_shape, ElementKind::Face);
        assert!(
            box_edges.iter().all(|edge| box_faces
                .iter()
                .all(|face| !backend.is_seam(&edge.shape, &face.shape).unwrap())),
            "a box has no seam edge"
        );
    }

    #[test]
    fn parameter_ranges_match_the_face_and_edge_geometry() {
        let backend = OcctBackend::new();
        let cylinder = backend
            .make_cylinder(5.0, 10.0, Vector3::zero(), Vector3::new(0.0, 0.0, 1.0))
            .expect("a cylinder");
        let face = backend.elements(&cylinder, ElementKind::Face)[0]
            .shape
            .clone();
        let (u0, u1, v0, v1) = backend.face_parameter_range(&face).unwrap();
        assert!((u1 - u0 - TAU).abs() < 1e-6, "a full turn in U");
        assert!((v1 - v0 - 10.0).abs() < 1e-6, "the cylinder's height in V");

        let box_shape = box_shape();
        let edge = backend.elements(&box_shape, ElementKind::Edge)[0]
            .shape
            .clone();
        let (first, last) = backend.edge_parameter_range(&edge).unwrap();
        assert!(first.is_finite() && last.is_finite());
        assert!(
            [2.0, 3.0, 4.0]
                .iter()
                .any(|length| ((last - first) - length).abs() < 1e-9),
            "a box edge spans one of the box's dimensions: {first}..{last}"
        );
    }

    #[test]
    fn is_equal_requires_the_same_orientation() {
        let backend = OcctBackend::new();
        let box_shape = box_shape();

        // A separate handle around the same TShape, location and orientation.
        let same = {
            let occt = downcast(&box_shape).unwrap();
            let guard = occt.borrow();
            erase(guard.as_ref().unwrap())
        };
        assert!(backend.is_equal(&box_shape, &same).unwrap());

        let reversed = backend.reversed(&box_shape).unwrap();
        assert!(
            backend.same_shape(&box_shape, &reversed),
            "reversing changes orientation only"
        );
        assert!(
            !backend.is_equal(&box_shape, &reversed).unwrap(),
            "isEqual sees the orientation difference"
        );
    }

    #[test]
    fn a_copy_has_a_new_tshape_and_the_same_geometry() {
        let backend = OcctBackend::new();
        let box_shape = box_shape();
        let copy = backend.copy_shape(&box_shape).expect("a deep copy");

        assert!(!backend.same_shape(&box_shape, &copy), "a new TShape");
        assert!(!backend.is_equal(&box_shape, &copy).unwrap());
        let original = backend.mass_properties(&box_shape).expect("box mass");
        let copied = backend.mass_properties(&copy).expect("copy mass");
        assert!(
            (original.volume - copied.volume).abs() < 1e-9,
            "the copy has the same volume"
        );
        assert!(
            (original.area - copied.area).abs() < 1e-9,
            "the copy has the same area"
        );
    }

    #[test]
    fn the_shape_hash_follows_is_same_not_orientation() {
        let backend = OcctBackend::new();
        let box_shape = box_shape();
        let hash = backend.shape_hash(&box_shape).unwrap();

        // Another handle around the same TShape hashes the same.
        let same = {
            let occt = downcast(&box_shape).unwrap();
            let guard = occt.borrow();
            erase(guard.as_ref().unwrap())
        };
        assert_eq!(backend.shape_hash(&same).unwrap(), hash);

        // Orientation is not part of `IsSame`, so it is not part of the hash.
        let reversed = backend.reversed(&box_shape).unwrap();
        assert_eq!(backend.shape_hash(&reversed).unwrap(), hash);

        // A deep copy is a different TShape and hashes differently.
        let copy = backend.copy_shape(&box_shape).unwrap();
        assert_ne!(backend.shape_hash(&copy).unwrap(), hash);
    }
}

#[cfg(test)]
mod geometry_query_tests {
    //! The closest-point, closedness, tolerance and parameter-evaluation
    //! queries (`Shape.distToShape`/`isClosed`/`getTolerance`, `Edge.valueAt`
    //! and `Face.valueAt`) against real OCCT primitives.

    use std::f64::consts::{FRAC_PI_2, PI};

    use ferrocad_curves::LineSegment;
    use ferrocad_geom::{ElementKind, GeometryBackend};
    use ferrocad_types::{Placement, Rotation};

    use super::*;

    fn placed_box(offset: Vector3) -> Shape {
        let backend = OcctBackend::new();
        backend
            .place(
                &backend.make_box(2.0, 2.0, 2.0).expect("a box"),
                &Placement {
                    base: offset,
                    rotation: Rotation::identity(),
                },
            )
            .expect("a placed box")
    }

    fn box_shape() -> Shape {
        OcctBackend::new().make_box(2.0, 3.0, 4.0).expect("a box")
    }

    /// A planar rectangular face on `z = 0`, from `(0, 0)` to `(width, height)`.
    fn rectangle_face(width: f64, height: f64) -> Shape {
        let backend = OcctBackend::new();
        let corners = [
            Vector3::new(0.0, 0.0, 0.0),
            Vector3::new(width, 0.0, 0.0),
            Vector3::new(width, height, 0.0),
            Vector3::new(0.0, height, 0.0),
        ];
        let mut edges = Vec::with_capacity(4);
        for i in 0..4 {
            let segment =
                LineSegment::new(corners[i], corners[(i + 1) % 4]).expect("a rectangle side");
            edges.push(
                backend
                    .make_edge(&EdgeCurve::LineSegment(segment))
                    .expect("an edge"),
            );
        }
        let wire = backend.make_wire(&edges).expect("a wire");
        backend.make_face(&[wire]).expect("a face")
    }

    fn close(a: Vector3, b: Vector3) -> bool {
        a.distance(&b) < 1e-7
    }

    // --- dist_to_shape ---------------------------------------------------

    #[test]
    fn two_boxes_five_apart_report_the_gap_and_its_endpoints() {
        let backend = OcctBackend::new();
        let first = placed_box(Vector3::zero());
        let second = placed_box(Vector3::new(7.0, 0.0, 0.0));
        let result = backend.dist_to_shape(&first, &second).unwrap();

        assert!((result.distance - 5.0).abs() < 1e-9, "{}", result.distance);
        assert!(close(
            result.first,
            Vector3::new(2.0, result.first.y, result.first.z)
        ));
        assert!(close(
            result.second,
            Vector3::new(7.0, result.first.y, result.first.z)
        ));
        assert!((result.first.distance(&result.second) - 5.0).abs() < 1e-9);
    }

    #[test]
    fn intersecting_boxes_are_zero_apart() {
        let backend = OcctBackend::new();
        let first = placed_box(Vector3::zero());
        let second = placed_box(Vector3::new(1.0, 1.0, 1.0));
        let result = backend.dist_to_shape(&first, &second).unwrap();

        assert!(result.distance.abs() < 1e-9, "{}", result.distance);
        assert!(close(result.first, result.second));
    }

    #[test]
    fn a_vertex_and_a_face_report_both_closest_points() {
        let backend = OcctBackend::new();
        let face = rectangle_face(2.0, 3.0);
        let vertex = backend
            .make_vertex(Vector3::new(1.0, 1.5, 5.0))
            .expect("a vertex");

        let result = backend.dist_to_shape(&vertex, &face).unwrap();
        assert!((result.distance - 5.0).abs() < 1e-9, "{}", result.distance);
        assert!(
            close(result.first, Vector3::new(1.0, 1.5, 5.0)),
            "{:?}",
            result.first
        );
        assert!(
            close(result.second, Vector3::new(1.0, 1.5, 0.0)),
            "{:?}",
            result.second
        );

        // The pair follows the operand order, so swapping swaps the points.
        let swapped = backend.dist_to_shape(&face, &vertex).unwrap();
        assert!(close(swapped.first, result.second));
        assert!(close(swapped.second, result.first));
        assert!((swapped.distance - result.distance).abs() < 1e-9);
    }

    // --- is_closed -------------------------------------------------------

    #[test]
    fn a_box_solid_is_closed() {
        let backend = OcctBackend::new();
        assert!(backend.is_closed(&box_shape()).unwrap());
    }

    #[test]
    fn an_open_shell_and_a_bare_face_are_not_closed() {
        let backend = OcctBackend::new();
        let box_shape = box_shape();
        let face = backend.elements(&box_shape, ElementKind::Face)[0]
            .shape
            .clone();
        assert!(!backend.is_closed(&face).unwrap(), "a bare face is open");

        let open_shell = backend
            .make_shell(std::slice::from_ref(&face))
            .expect("a one-face shell");
        assert!(
            !backend.is_closed(&open_shell).unwrap(),
            "one face leaves free boundaries"
        );
    }

    #[test]
    fn a_wire_and_an_edge_are_closed_when_they_have_no_free_ends() {
        let backend = OcctBackend::new();

        let circle =
            Circle::new(Vector3::zero(), Vector3::new(0.0, 0.0, 1.0), 2.0).expect("a circle");
        let circle_edge = backend
            .make_edge(&EdgeCurve::Circle(circle))
            .expect("a circle edge");
        assert!(
            backend.is_closed(&circle_edge).unwrap(),
            "a circle edge closes on itself"
        );
        let closed_wire = backend
            .make_wire(std::slice::from_ref(&circle_edge))
            .expect("a circle wire");
        assert!(backend.is_closed(&closed_wire).unwrap());

        let segment =
            LineSegment::new(Vector3::zero(), Vector3::new(1.0, 0.0, 0.0)).expect("a segment");
        let segment_edge = backend
            .make_edge(&EdgeCurve::LineSegment(segment))
            .expect("a segment edge");
        assert!(!backend.is_closed(&segment_edge).unwrap());
        let open_wire = backend
            .make_wire(std::slice::from_ref(&segment_edge))
            .expect("a segment wire");
        assert!(!backend.is_closed(&open_wire).unwrap());
    }

    #[test]
    fn a_cylinders_shell_is_closed() {
        let backend = OcctBackend::new();
        let cylinder = backend
            .make_cylinder(5.0, 10.0, Vector3::zero(), Vector3::new(0.0, 0.0, 1.0))
            .expect("a cylinder");
        let shells = backend.elements(&cylinder, ElementKind::Shell);
        assert_eq!(shells.len(), 1);
        assert!(backend.is_closed(&shells[0].shape).unwrap());
    }

    // --- shape_tolerance -------------------------------------------------

    #[test]
    fn a_fresh_box_reports_a_positive_kernel_tolerance() {
        let backend = OcctBackend::new();
        let box_shape = box_shape();
        let tolerance = backend.shape_tolerance(&box_shape).unwrap();
        assert!(tolerance.is_finite() && tolerance > 0.0, "{tolerance}");

        for kind in [ElementKind::Vertex, ElementKind::Edge, ElementKind::Face] {
            let element = &backend.elements(&box_shape, kind)[0].shape;
            let tolerance = backend.shape_tolerance(element).unwrap();
            assert!(
                tolerance.is_finite() && tolerance > 0.0,
                "{kind:?}: {tolerance}"
            );
        }
    }

    // --- value_at --------------------------------------------------------

    #[test]
    fn a_box_edge_maps_its_parameter_endpoints_to_vertices() {
        let backend = OcctBackend::new();
        let unit = backend.make_box(1.0, 1.0, 1.0).expect("a unit box");
        let edge = backend.elements(&unit, ElementKind::Edge)[0].shape.clone();
        let (first, last) = backend.edge_parameter_range(&edge).unwrap();
        let start = backend.edge_value_at(&edge, first).unwrap();
        let end = backend.edge_value_at(&edge, last).unwrap();

        let vertices: Vec<Vector3> = backend
            .elements(&unit, ElementKind::Vertex)
            .iter()
            .map(|vertex| backend.vertex_point(&vertex.shape).expect("a vertex point"))
            .collect();
        assert!(
            vertices.iter().any(|vertex| close(*vertex, start)),
            "{start:?}"
        );
        assert!(vertices.iter().any(|vertex| close(*vertex, end)), "{end:?}");
        assert!((start.distance(&end) - 1.0).abs() < 1e-9);

        // A line's parameter is arc length, so the midpoint is the midpoint.
        let middle = backend.edge_value_at(&edge, 0.5 * (first + last)).unwrap();
        let expected = start.add(&end).scale(0.5);
        assert!(close(middle, expected), "{middle:?} != {expected:?}");
    }

    #[test]
    fn a_plane_face_evaluates_the_expected_point() {
        let backend = OcctBackend::new();
        let face = rectangle_face(2.0, 3.0);
        let (u0, u1, v0, v1) = backend.face_parameter_range(&face).unwrap();

        // A plane is affine in (u, v), so the middle of the parameter bounds is
        // the middle of the rectangle.
        let center = backend
            .face_value_at(&face, 0.5 * (u0 + u1), 0.5 * (v0 + v1))
            .unwrap();
        assert!(close(center, Vector3::new(1.0, 1.5, 0.0)), "{center:?}");

        let corners = [
            Vector3::new(0.0, 0.0, 0.0),
            Vector3::new(2.0, 0.0, 0.0),
            Vector3::new(2.0, 3.0, 0.0),
            Vector3::new(0.0, 3.0, 0.0),
        ];
        for (u, v) in [(u0, v0), (u1, v0), (u1, v1), (u0, v1)] {
            let point = backend.face_value_at(&face, u, v).unwrap();
            assert!(
                corners.iter().any(|corner| close(*corner, point)),
                "{point:?} is not a rectangle corner"
            );
        }
    }

    #[test]
    fn a_sphere_face_evaluates_its_parameters_and_poles() {
        let backend = OcctBackend::new();
        let radius = 2.0;
        let sphere = backend
            .make_sphere(radius, Vector3::zero())
            .expect("a sphere");
        let face = backend.elements(&sphere, ElementKind::Face)[0]
            .shape
            .clone();
        let (u0, u1, v0, v1) = backend.face_parameter_range(&face).unwrap();

        // `Geom_SphericalSurface`: P(u, v) = R * (cos v cos u, cos v sin u, sin v)
        // with u in [0, 2*pi] and v in [-pi/2, pi/2].
        assert!((u0 - 0.0).abs() < 1e-9, "u0 = {u0}");
        assert!((u1 - 2.0 * PI).abs() < 1e-9, "u1 = {u1}");
        assert!((v0 + FRAC_PI_2).abs() < 1e-9, "v0 = {v0}");
        assert!((v1 - FRAC_PI_2).abs() < 1e-9, "v1 = {v1}");

        // (0, 0) is on the equator, on the surface's X axis.
        let equator = backend.face_value_at(&face, 0.0, 0.0).unwrap();
        assert!(
            close(equator, Vector3::new(radius, 0.0, 0.0)),
            "{equator:?}"
        );

        // Every u at v = +pi/2 is the north pole, every u at v = -pi/2 the south.
        let north = backend.face_value_at(&face, 0.0, v1).unwrap();
        assert!(close(north, Vector3::new(0.0, 0.0, radius)), "{north:?}");
        let south = backend.face_value_at(&face, PI, v0).unwrap();
        assert!(close(south, Vector3::new(0.0, 0.0, -radius)), "{south:?}");
    }

    #[test]
    fn value_at_rejects_the_wrong_shape_type() {
        let backend = OcctBackend::new();
        let box_shape = box_shape();
        let face = backend.elements(&box_shape, ElementKind::Face)[0]
            .shape
            .clone();
        let edge = backend.elements(&box_shape, ElementKind::Edge)[0]
            .shape
            .clone();

        let error = backend.edge_value_at(&face, 0.0).unwrap_err();
        assert!(matches!(
            error.downcast_ref::<OcctError>(),
            Some(OcctError::InvalidInput { .. })
        ));

        let error = backend.face_value_at(&edge, 0.0, 0.0).unwrap_err();
        assert!(matches!(
            error.downcast_ref::<OcctError>(),
            Some(OcctError::InvalidInput { .. })
        ));
    }

    // --- surface_data ----------------------------------------------------

    /// The `(u, v)` a surface's own frame assigns to `point`, matching the
    /// formulas `Part.Plane`/`Cylinder`/`Sphere.parameter` use.
    fn parameters(surface: &SurfaceData, point: Vector3) -> (f64, f64) {
        match surface {
            SurfaceData::Plane {
                origin,
                x_dir,
                y_dir,
                ..
            } => {
                let offset = point.sub(origin);
                (offset.dot(x_dir), offset.dot(y_dir))
            }
            SurfaceData::Cylinder {
                origin,
                axis,
                x_dir,
                direct,
                ..
            } => {
                let offset = point.sub(origin);
                let mut y_dir = axis.cross(x_dir);
                if !direct {
                    y_dir = y_dir.neg();
                }
                let mut u = offset.dot(&y_dir).atan2(offset.dot(x_dir));
                if u < 0.0 {
                    u += std::f64::consts::TAU;
                }
                (u, offset.dot(axis))
            }
            SurfaceData::Sphere {
                center,
                axis,
                x_dir,
                radius,
                direct,
            } => {
                let offset = point.sub(center);
                let mut y_dir = axis.cross(x_dir);
                if !direct {
                    y_dir = y_dir.neg();
                }
                let mut u = offset.dot(&y_dir).atan2(offset.dot(x_dir));
                if u < 0.0 {
                    u += std::f64::consts::TAU;
                }
                let v = (offset.dot(axis) / radius).clamp(-1.0, 1.0).asin();
                (u, v)
            }
            SurfaceData::Cone {
                origin,
                axis,
                x_dir,
                semi_angle,
                direct,
                ..
            } => {
                let offset = point.sub(origin);
                let mut y_dir = axis.cross(x_dir);
                if !direct {
                    y_dir = y_dir.neg();
                }
                let axial = offset.dot(axis);
                let radial = offset.sub(&axis.scale(axial));
                let mut u = radial.dot(&y_dir).atan2(radial.dot(x_dir));
                if u < 0.0 {
                    u += std::f64::consts::TAU;
                }
                (u, axial / semi_angle.cos())
            }
            SurfaceData::Torus {
                center,
                axis,
                x_dir,
                major_radius,
                direct,
                minor_radius: _,
            } => {
                let offset = point.sub(center);
                let mut y_dir = axis.cross(x_dir);
                if !direct {
                    y_dir = y_dir.neg();
                }
                let height = offset.dot(axis);
                let radial = offset.sub(&axis.scale(height));
                let mut u = radial.dot(&y_dir).atan2(radial.dot(x_dir));
                if u < 0.0 {
                    u += std::f64::consts::TAU;
                }
                let v = height.atan2(radial.length() - major_radius);
                (u, v)
            }
            SurfaceData::Other { .. } => panic!("a classified surface was expected"),
        }
    }

    #[test]
    fn a_box_face_is_a_plane_and_parameter_round_trips_exactly() {
        let backend = OcctBackend::new();
        let shape = box_shape();
        let face = backend.elements(&shape, ElementKind::Face)[0]
            .shape
            .clone();
        let surface = backend.surface_data(&face).expect("a box face has a surface");
        let SurfaceData::Plane {
            origin,
            x_dir,
            y_dir,
            normal,
        } = &surface
        else {
            panic!("a box face is planar, got {surface:?}");
        };

        // The frame is orthonormal and right-handed.
        assert!((x_dir.length() - 1.0).abs() < 1e-12);
        assert!((y_dir.length() - 1.0).abs() < 1e-12);
        assert!((normal.length() - 1.0).abs() < 1e-12);
        assert!(x_dir.dot(y_dir).abs() < 1e-12);
        assert!(normal.sub(&x_dir.cross(y_dir)).length() < 1e-12);

        // A point on the face's own parameter grid comes back exactly.
        let (u, v) = (0.3, 0.7);
        let point = backend.face_value_at(&face, u, v).unwrap();
        let expected = origin.add(&x_dir.scale(u)).add(&y_dir.scale(v));
        assert!(close(point, expected), "{point:?} != {expected:?}");
        let (u2, v2) = parameters(&surface, point);
        assert!((u2 - u).abs() < 1e-12, "u = {u2}");
        assert!((v2 - v).abs() < 1e-12, "v = {v2}");
        assert!(close(backend.face_value_at(&face, u2, v2).unwrap(), point));
    }

    #[test]
    fn a_cylinder_face_is_classified_and_parameter_round_trips() {
        let backend = OcctBackend::new();
        let shape = backend
            .make_cylinder(2.0, 5.0, Vector3::zero(), Vector3::new(0.0, 0.0, 1.0))
            .expect("a cylinder");
        let faces = backend.elements(&shape, ElementKind::Face);
        let (face, surface) = faces
            .iter()
            .find_map(|face| match backend.surface_data(&face.shape) {
                Some(surface @ SurfaceData::Cylinder { .. }) => {
                    Some((face.shape.clone(), surface))
                }
                _ => None,
            })
            .expect("a cylindrical lateral face");
        let SurfaceData::Cylinder {
            origin,
            axis,
            x_dir,
            radius,
            ..
        } = &surface
        else {
            unreachable!();
        };
        assert!((radius - 2.0).abs() < 1e-12);
        assert!(axis.sub(&Vector3::new(0.0, 0.0, 1.0)).length() < 1e-12);
        assert!((origin.length()).abs() < 1e-12);
        assert!(x_dir.dot(axis).abs() < 1e-12);

        for (u, v) in [(0.0, 0.5), (0.7, 2.0), (5.5, 4.0)] {
            let point = backend.face_value_at(&face, u, v).unwrap();
            let (u2, v2) = parameters(&surface, point);
            assert!((u2 - u).abs() < 1e-9, "u {u} -> {u2}");
            assert!((v2 - v).abs() < 1e-9, "v {v} -> {v2}");
            let back = backend.face_value_at(&face, u2, v2).unwrap();
            assert!(close(back, point), "{back:?} != {point:?}");
        }
    }

    #[test]
    fn a_sphere_face_is_classified_and_parameter_round_trips() {
        let backend = OcctBackend::new();
        let shape = backend
            .make_sphere(2.0, Vector3::new(1.0, 2.0, 3.0))
            .expect("a sphere");
        let face = backend.elements(&shape, ElementKind::Face)[0]
            .shape
            .clone();
        let surface = backend.surface_data(&face).expect("a sphere face has a surface");
        let SurfaceData::Sphere {
            center,
            axis,
            x_dir,
            radius,
            ..
        } = &surface
        else {
            panic!("a sphere face is spherical, got {surface:?}");
        };
        assert!((radius - 2.0).abs() < 1e-12);
        assert!(center.sub(&Vector3::new(1.0, 2.0, 3.0)).length() < 1e-12);
        assert!(axis.sub(&Vector3::new(0.0, 0.0, 1.0)).length() < 1e-12);
        assert!(x_dir.dot(axis).abs() < 1e-12);

        for (u, v) in [(0.0, 0.0), (0.8, 0.4), (4.0, -1.2)] {
            let point = backend.face_value_at(&face, u, v).unwrap();
            let (u2, v2) = parameters(&surface, point);
            assert!((u2 - u).abs() < 1e-9, "u {u} -> {u2}");
            assert!((v2 - v).abs() < 1e-9, "v {v} -> {v2}");
            let back = backend.face_value_at(&face, u2, v2).unwrap();
            assert!(close(back, point), "{back:?} != {point:?}");
        }
    }

    #[test]
    fn cone_and_torus_faces_are_classified_and_parameter_round_trips() {
        let backend = OcctBackend::new();
        let cone = backend
            .make_cone(
                2.0,
                1.0,
                4.0,
                Vector3::zero(),
                Vector3::new(0.0, 0.0, 1.0),
            )
            .expect("a cone");
        let faces = backend.elements(&cone, ElementKind::Face);
        let (face, surface) = faces
            .iter()
            .find_map(|face| match backend.surface_data(&face.shape) {
                Some(surface @ SurfaceData::Cone { .. }) => Some((face.shape.clone(), surface)),
                _ => None,
            })
            .expect("a conical face");
        let SurfaceData::Cone {
            origin,
            axis,
            x_dir,
            radius,
            semi_angle,
            ..
        } = &surface
        else {
            unreachable!();
        };
        assert!((radius - 2.0).abs() < 1e-9);
        assert!(semi_angle.is_finite() && semi_angle.abs() > 0.0);
        assert!(x_dir.dot(axis).abs() < 1e-12);
        let _ = origin;

        for (u, v) in [(0.0, 0.0), (0.8, 1.5), (4.0, 3.0)] {
            let point = backend.face_value_at(&face, u, v).unwrap();
            let (u2, v2) = parameters(&surface, point);
            assert!((u2 - u).abs() < 1e-9, "u {u} -> {u2}");
            assert!((v2 - v).abs() < 1e-9, "v {v} -> {v2}");
            let back = backend.face_value_at(&face, u2, v2).unwrap();
            assert!(close(back, point), "{back:?} != {point:?}");
        }

        // A non-face reports no surface at all.
        let edge = backend.elements(&cone, ElementKind::Edge)[0].shape.clone();
        assert!(backend.surface_data(&edge).is_none());
    }
}

#[cfg(test)]
mod tessellation_tests {
    //! Both deflections decide the mesh. OCCT only ever received the linear value,
    //! so `Shape.tessellate(deflection, angular)` ignored `angular` and every coarse
    //! deflection of a sphere produced the same 304-triangle mesh.

    use ferrocad_geom::TessellationOptions;

    use super::*;

    fn sphere() -> Shape {
        OcctBackend::new()
            .make_sphere(10.0, Vector3::new(0.0, 0.0, 0.0))
            .expect("a sphere")
    }

    fn triangle_count(linear: f64, angular: f64) -> usize {
        OcctBackend::new()
            .tessellate(
                &sphere(),
                &TessellationOptions {
                    linear_deflection: Some(linear),
                    angular_deflection: angular,
                    relative: false,
                },
            )
            .expect("a tessellation")
            .triangles
            .len()
            / 3
    }

    #[test]
    fn angular_deflection_changes_the_triangle_count() {
        // A fresh shape each time: OCCT stores the triangulation on the shape and
        // keeps it when the next request is not finer, so reusing one shape would
        // measure the cache, not the deflection.
        let coarse = triangle_count(2.0, 2.0);
        // The display mesh keeps the two degenerate pole triangles the Python
        // `tessellate` weld drops, so this is 48, not the 46 facets a script sees.
        assert!(coarse < 100, "a coarse sphere must be small, got {coarse}");
        assert!(
            triangle_count(2.0, 1.0) > coarse,
            "a smaller angular deflection must refine the mesh"
        );
        assert!(
            triangle_count(0.5, 0.2) > triangle_count(2.0, 1.0),
            "a tighter linear deflection must refine the mesh"
        );
    }

    #[test]
    fn a_non_positive_angular_deflection_is_rejected() {
        let shape = sphere();
        for angular in [0.0, -0.5, f64::NAN, std::f64::consts::PI] {
            let result = OcctBackend::new().tessellate(
                &shape,
                &TessellationOptions {
                    linear_deflection: Some(1.0),
                    angular_deflection: angular,
                    relative: false,
                },
            );
            assert!(result.is_err(), "angular {angular} must be rejected");
        }
    }
}
