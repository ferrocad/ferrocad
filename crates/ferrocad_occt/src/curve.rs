//! Typed B-spline state and the curve classification/2-D lifting that used to
//! live in `ferrocad_part_cxx`.
//!
//! The old bridge flattened a B-spline into one `Vec<f64>` and a curve's dynamic
//! type into a 14-value header plus a tail. This module reads the same OCCT
//! accessors directly and returns [`BsplineState`] / [`ferrocad_curves::EdgeCurve`],
//! so a caller can never disagree with the curve `toShape` would build.

use cxx::UniquePtr;
use ferrocad_curves::{
    Arc, ArcOfHyperbola, ArcOfParabola, BezierCurve, Circle, EdgeCurve, Ellipse, EllipticalArc,
    Hyperbola, Line, NurbsCurve, OffsetCurve, Parabola, Vector3,
};
use ferrocad_geom::{GeomError, SurfaceSpec};
use opencascade_sys as ffi;

use ffi::top_abs::TopAbs_ShapeEnum;
use ffi::topo_ds::{TopoDS, TopoDS_Edge, TopoDS_Face, TopoDS_Shape};

use crate::error::OcctError;

/// The state of a `Geom_BSplineCurve`, read back from OCCT so every query agrees
/// with the curve `toShape` builds. This replaces the flat `Vec<f64>` layout the
/// bridge wrote (`fc_bspline_push_state`).
#[derive(Clone, Debug)]
pub struct BsplineState {
    pub degree: i32,
    pub max_degree: i32,
    pub periodic: bool,
    pub rational: bool,
    pub start: Vector3,
    pub end: Vector3,
    pub first_parameter: f64,
    pub last_parameter: f64,
    pub first_u_knot: i32,
    pub last_u_knot: i32,
    /// `GeomAbs_Shape` numeric value (0 = C0, 6 = CN).
    pub continuity: i32,
    pub knot_sequence: Vec<f64>,
    pub knots: Vec<f64>,
    pub mults: Vec<i32>,
    pub poles: Vec<Vector3>,
    pub weights: Vec<f64>,
}

impl BsplineState {
    /// Rebuild `checkRational` for the stored curve: a rational curve must keep
    /// its weights, a non-rational one is reconstructed from unit weights.
    pub fn check_rational(&self) -> bool {
        !self.rational
    }
}

fn point(pnt: &ffi::gp::gp_Pnt) -> Vector3 {
    Vector3::new(pnt.X(), pnt.Y(), pnt.Z())
}

fn direction(dir: &ffi::gp::gp_Dir) -> Vector3 {
    Vector3::new(dir.X(), dir.Y(), dir.Z())
}

fn unsupported(what: &'static str) -> GeomError {
    OcctError::InvalidInput { what }.into()
}

fn continuity_value(continuity: ffi::geom_api::GeomAbs_Shape) -> i32 {
    match continuity {
        ffi::geom_api::GeomAbs_Shape::GeomAbs_C0 => 0,
        ffi::geom_api::GeomAbs_Shape::GeomAbs_G1 => 1,
        ffi::geom_api::GeomAbs_Shape::GeomAbs_C1 => 2,
        ffi::geom_api::GeomAbs_Shape::GeomAbs_G2 => 3,
        ffi::geom_api::GeomAbs_Shape::GeomAbs_C2 => 4,
        ffi::geom_api::GeomAbs_Shape::GeomAbs_C3 => 5,
        ffi::geom_api::GeomAbs_Shape::GeomAbs_CN => 6,
        _ => 6,
    }
}

/// Build a B-spline curve handle from explicit poles, weights, knots and
/// multiplicities (`Geom_BSplineCurve`'s full constructor). `None` when the
/// arguments do not describe a valid curve.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build(
    coords: &[f64],
    weights: &[f64],
    knots: &[f64],
    multiplicities: &[i32],
    degree: i32,
    periodic: bool,
    check_rational: bool,
) -> Option<UniquePtr<ffi::geom::Handle_Geom_BSplineCurve>> {
    let pole_count = coords.len() / 3;
    let knot_count = knots.len();
    if pole_count < 2
        || knot_count < 1
        || degree < 1
        || degree > ffi::geom::HandleGeomBSplineCurve_MaxDegree()
        || weights.len() != pole_count
        || multiplicities.len() != knot_count
    {
        return None;
    }

    let mut poles = ffi::t_col_gp::TColgp_Array1OfPnt_new(1, pole_count as i32);
    let mut pole_weights = ffi::t_col_std::TColStd_Array1OfReal_new(1, pole_count as i32);
    for i in 0..pole_count {
        let pnt = ffi::gp::new_point(coords[3 * i], coords[3 * i + 1], coords[3 * i + 2]);
        poles.pin_mut().SetValue(i as i32 + 1, &pnt);
        ffi::t_col_std::TColStd_Array1OfReal_SetValue(
            pole_weights.pin_mut(),
            i as i32 + 1,
            weights[i],
        );
    }
    let mut knot_values = ffi::t_col_std::TColStd_Array1OfReal_new(1, knot_count as i32);
    let mut knot_mults = ffi::t_col_std::TColStd_Array1OfInteger_new(1, knot_count as i32);
    for i in 0..knot_count {
        ffi::t_col_std::TColStd_Array1OfReal_SetValue(
            knot_values.pin_mut(),
            i as i32 + 1,
            knots[i],
        );
        ffi::t_col_std::TColStd_Array1OfInteger_SetValue(
            knot_mults.pin_mut(),
            i as i32 + 1,
            multiplicities[i],
        );
    }
    ffi::geom::Geom_BSplineCurve_new_full(
        &poles,
        &pole_weights,
        &knot_values,
        &knot_mults,
        degree,
        periodic,
        check_rational,
    )
    .ok()
}

/// Read the full state of a B-spline back from OCCT.
pub(crate) fn state(curve: &ffi::geom::Handle_Geom_BSplineCurve) -> Option<BsplineState> {
    let degree = ffi::geom::HandleGeomBSplineCurve_Degree(curve);
    let max_degree = ffi::geom::HandleGeomBSplineCurve_MaxDegree();
    let nb_poles = ffi::geom::HandleGeomBSplineCurve_NbPoles(curve);
    let nb_knots = ffi::geom::HandleGeomBSplineCurve_NbKnots(curve);
    let periodic = ffi::geom::HandleGeomBSplineCurve_IsPeriodic(curve);
    let rational = ffi::geom::HandleGeomBSplineCurve_IsRational(curve);

    let start = ffi::geom::HandleGeomBSplineCurve_StartPoint(curve);
    let end = ffi::geom::HandleGeomBSplineCurve_EndPoint(curve);
    let continuity = ffi::geom::HandleGeomBSplineCurve_Continuity(curve);

    let first_parameter = ffi::geom::HandleGeomCurve_FirstParameter(
        ffi::geom::new_HandleGeomCurve_from_HandleGeom_BSplineCurve(curve)
            .as_ref()
            .unwrap(),
    );
    let last_parameter = ffi::geom::HandleGeomCurve_LastParameter(
        ffi::geom::new_HandleGeomCurve_from_HandleGeom_BSplineCurve(curve)
            .as_ref()
            .unwrap(),
    );

    let sequence = ffi::geom::HandleGeomBSplineCurve_KnotSequence(curve);
    let sequence_length = sequence.Length();
    let mut knot_sequence = Vec::with_capacity(sequence_length as usize);
    for i in 1..=sequence_length {
        knot_sequence.push(ffi::t_col_std::TColStd_Array1OfReal_Value(&sequence, i));
    }

    let mut knots = Vec::with_capacity(nb_knots as usize);
    let mut mults = Vec::with_capacity(nb_knots as usize);
    for i in 1..=nb_knots {
        knots.push(ffi::geom::HandleGeomBSplineCurve_Knot(curve, i));
        mults.push(ffi::geom::HandleGeomBSplineCurve_Multiplicity(curve, i));
    }

    let mut poles = Vec::with_capacity(nb_poles as usize);
    let mut weights = Vec::with_capacity(nb_poles as usize);
    for i in 1..=nb_poles {
        poles.push(point(&ffi::geom::HandleGeomBSplineCurve_Pole(curve, i)));
        weights.push(ffi::geom::HandleGeomBSplineCurve_Weight(curve, i));
    }

    Some(BsplineState {
        degree,
        max_degree,
        periodic,
        rational,
        start: point(&start),
        end: point(&end),
        first_parameter,
        last_parameter,
        first_u_knot: ffi::geom::HandleGeomBSplineCurve_FirstUKnotIndex(curve),
        last_u_knot: ffi::geom::HandleGeomBSplineCurve_LastUKnotIndex(curve),
        continuity: continuity_value(continuity),
        knot_sequence,
        knots,
        mults,
        poles,
        weights,
    })
}

/// `Geom_BSplineCurve` from explicit poles/weights/knots/mults, reported as state.
#[allow(clippy::too_many_arguments)]
pub(crate) fn probe(
    coords: &[f64],
    weights: &[f64],
    knots: &[f64],
    multiplicities: &[i32],
    degree: i32,
    periodic: bool,
    check_rational: bool,
) -> Option<BsplineState> {
    let curve = build(coords, weights, knots, multiplicities, degree, periodic, check_rational)?;
    state(&curve)
}

/// The state of the B-spline interpolating `coords` (`GeomAPI_Interpolate`).
pub(crate) fn interpolated(coords: &[f64], periodic: bool) -> Option<BsplineState> {
    let count = coords.len() / 3;
    if count < 2 {
        return None;
    }
    let mut points = ffi::t_col_gp::TColgp_HArray1OfPnt_new(1, count as i32);
    for i in 0..count {
        let pnt = ffi::gp::new_point(coords[3 * i], coords[3 * i + 1], coords[3 * i + 2]);
        points.pin_mut().SetValue(i as i32 + 1, &pnt);
    }
    let points = ffi::t_col_gp::new_HandleTColgpHArray1OfPnt_from_TColgpHArray1OfPnt(points);
    let mut interpolate = ffi::geom_api::GeomAPI_Interpolate_new(&points, periodic, 1e-7);
    interpolate.pin_mut().Perform();
    if !interpolate.IsDone() {
        return None;
    }
    let curve = ffi::geom_api::GeomAPI_Interpolate_Curve(&interpolate);
    if curve.IsNull() {
        return None;
    }
    state(&curve)
}

/// Rebuild the curve in (or out of) the periodic form and return its new state.
#[allow(clippy::too_many_arguments)]
pub(crate) fn set_periodic(
    coords: &[f64],
    weights: &[f64],
    knots: &[f64],
    multiplicities: &[i32],
    degree: i32,
    periodic: bool,
    check_rational: bool,
    target_periodic: bool,
) -> Option<BsplineState> {
    let mut curve = build(coords, weights, knots, multiplicities, degree, periodic, check_rational)?;
    if target_periodic {
        ffi::geom::HandleGeomBSplineCurve_SetPeriodic(curve.pin_mut()).ok()?;
    } else {
        ffi::geom::HandleGeomBSplineCurve_SetNotPeriodic(curve.pin_mut());
    }
    state(&curve)
}

/// `BSplineCurve.segment(u1, u2)`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn segment(
    coords: &[f64],
    weights: &[f64],
    knots: &[f64],
    multiplicities: &[i32],
    degree: i32,
    periodic: bool,
    check_rational: bool,
    u1: f64,
    u2: f64,
) -> Option<BsplineState> {
    let mut curve = build(coords, weights, knots, multiplicities, degree, periodic, check_rational)?;
    ffi::geom::HandleGeomBSplineCurve_Segment(curve.pin_mut(), u1, u2).ok()?;
    let first = ffi::geom::HandleGeomCurve_FirstParameter(
        ffi::geom::new_HandleGeomCurve_from_HandleGeom_BSplineCurve(&curve)
            .as_ref()
            .unwrap(),
    );
    let last = ffi::geom::HandleGeomCurve_LastParameter(
        ffi::geom::new_HandleGeomCurve_from_HandleGeom_BSplineCurve(&curve)
            .as_ref()
            .unwrap(),
    );
    let tolerance = ffi::precision::Precision::Confusion();
    if (first - u1).abs() > tolerance || (last - u2).abs() > tolerance {
        return None;
    }
    state(&curve)
}

/// `Geom_BSplineCurve::Resolution` for a 3D tolerance.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolution(
    coords: &[f64],
    weights: &[f64],
    knots: &[f64],
    multiplicities: &[i32],
    degree: i32,
    periodic: bool,
    check_rational: bool,
    tolerance: f64,
) -> Option<f64> {
    let curve = build(coords, weights, knots, multiplicities, degree, periodic, check_rational)?;
    let value = ffi::geom::HandleGeomBSplineCurve_Resolution(&curve, tolerance);
    value.is_finite().then_some(value)
}

/// The point at parameter `u` on the B-spline.
#[allow(clippy::too_many_arguments)]
pub(crate) fn bspline_point(
    coords: &[f64],
    weights: &[f64],
    knots: &[f64],
    multiplicities: &[i32],
    degree: i32,
    periodic: bool,
    check_rational: bool,
    u: f64,
) -> Option<Vector3> {
    let curve = build(coords, weights, knots, multiplicities, degree, periodic, check_rational)?;
    Some(point(&ffi::geom::HandleGeomBSplineCurve_Value(&curve, u)))
}

/// A conic over `[first, last]` as a B-spline state.
#[allow(clippy::too_many_arguments)]
pub(crate) fn conic_to_spline(
    kind: i32,
    center: Vector3,
    normal: Vector3,
    x_axis: Vector3,
    major: f64,
    minor: f64,
    first: f64,
    last: f64,
    nurbs: bool,
) -> Option<BsplineState> {
    let origin = ffi::gp::new_point(center.x, center.y, center.z);
    let main_dir = ffi::gp::gp_Dir_new(normal.x, normal.y, normal.z);
    let x_dir = ffi::gp::gp_Dir_new(x_axis.x, x_axis.y, x_axis.z);
    let axis = ffi::gp::gp_Ax2_new3(&origin, &main_dir, &x_dir).ok()?;
    let conic = if kind == 2 {
        ffi::geom::new_HandleGeomCurve_from_HandleGeom_Circle(
            ffi::geom::Geom_Circle_new(&axis, major).as_ref().unwrap(),
        )
    } else {
        ffi::geom::new_HandleGeomCurve_from_HandleGeom_Ellipse(
            ffi::geom::Geom_Ellipse_new(&axis, major, minor).ok()?.as_ref().unwrap(),
        )
    };

    let mut spline = if nurbs {
        let trimmed = ffi::geom::Geom_TrimmedCurve_new(&conic, first, last).ok()?;
        let trimmed = ffi::geom::new_HandleGeomCurve_from_HandleGeom_TrimmedCurve(&trimmed);
        let mut spline = ffi::geom_convert::GeomConvert_CurveToBSplineCurve(&trimmed).ok()?;
        let fnew = ffi::geom::HandleGeomCurve_FirstParameter(
            ffi::geom::new_HandleGeomCurve_from_HandleGeom_BSplineCurve(&spline)
                .as_ref()
                .unwrap(),
        );
        let lnew = ffi::geom::HandleGeomCurve_LastParameter(
            ffi::geom::new_HandleGeomCurve_from_HandleGeom_BSplineCurve(&spline)
                .as_ref()
                .unwrap(),
        );
        if !ffi::geom::HandleGeomBSplineCurve_IsPeriodic(&spline) {
            let utol = ffi::geom::HandleGeomBSplineCurve_Resolution(
                &spline,
                ffi::precision::Precision::Confusion(),
            );
            if (first - fnew).abs() > utol || (last - lnew).abs() > utol {
                let nb_knots = ffi::geom::HandleGeomBSplineCurve_NbKnots(&spline);
                let mut knots = ffi::t_col_std::TColStd_Array1OfReal_new(1, nb_knots);
                for i in 1..=nb_knots {
                    ffi::t_col_std::TColStd_Array1OfReal_SetValue(
                        knots.pin_mut(),
                        i,
                        ffi::geom::HandleGeomBSplineCurve_Knot(&spline, i),
                    );
                }
                ffi::bspl_clib::BSplCLib_Reparametrize(first, last, knots.pin_mut()).ok()?;
                ffi::geom::HandleGeomBSplineCurve_SetKnots(spline.pin_mut(), &knots).ok()?;
            }
        }
        spline
    } else {
        let converter = ffi::shape_construct::ShapeConstruct_Curve_new();
        ffi::shape_construct::ShapeConstruct_Curve_ConvertToBSpline(
            &converter,
            &conic,
            first,
            last,
            ffi::precision::Precision::Confusion(),
        )
        .ok()?
    };

    if spline.IsNull() {
        return None;
    }
    state(&spline)
}

/// `Geom_BSplineCurve::RemoveKnot`, reported as the new state.
#[allow(clippy::too_many_arguments)]
pub(crate) fn remove_knot(
    coords: &[f64],
    weights: &[f64],
    knots: &[f64],
    multiplicities: &[i32],
    degree: i32,
    periodic: bool,
    check_rational: bool,
    index: i32,
    mult: i32,
    tolerance: f64,
) -> Option<BsplineState> {
    let mut curve = build(coords, weights, knots, multiplicities, degree, periodic, check_rational)?;
    if !ffi::geom::HandleGeomBSplineCurve_RemoveKnot(curve.pin_mut(), index, mult, tolerance).ok()? {
        return None;
    }
    state(&curve)
}

/// `GeomAPI_PointsToBSpline`, reported as state.
pub(crate) fn approximated(
    coords: &[f64],
    degree_min: i32,
    degree_max: i32,
    tolerance: f64,
) -> Option<BsplineState> {
    let count = coords.len() / 3;
    if count < 2 {
        return None;
    }
    let approximation = ffi::geom_api::GeomAPI_PointsToBSpline_new(
        coords,
        degree_min,
        degree_max,
        ffi::geom_api::GeomAbs_Shape::GeomAbs_C2,
        tolerance,
    )
    .ok()?;
    if !approximation.IsDone() {
        return None;
    }
    state(&ffi::geom_api::GeomAPI_PointsToBSpline_Curve(&approximation))
}

/// `Geom_BSplineCurve::InsertKnot`, reported as the new state.
#[allow(clippy::too_many_arguments)]
pub(crate) fn insert_knot(
    coords: &[f64],
    weights: &[f64],
    knots: &[f64],
    multiplicities: &[i32],
    degree: i32,
    periodic: bool,
    check_rational: bool,
    u: f64,
    mult: i32,
    tolerance: f64,
) -> Option<BsplineState> {
    let mut curve = build(coords, weights, knots, multiplicities, degree, periodic, check_rational)?;
    if mult < 1 {
        return None;
    }
    ffi::geom::HandleGeomBSplineCurve_InsertKnot(curve.pin_mut(), u, mult, tolerance);
    state(&curve)
}

/// `GeomAPI_ProjectPointOnCurve` for the curve described by the arrays.
#[allow(clippy::too_many_arguments)]
pub(crate) fn parameter(
    coords: &[f64],
    weights: &[f64],
    knots: &[f64],
    multiplicities: &[i32],
    degree: i32,
    periodic: bool,
    check_rational: bool,
    x: f64,
    y: f64,
    z: f64,
) -> Option<f64> {
    let curve = build(coords, weights, knots, multiplicities, degree, periodic, check_rational)?;
    let curve = ffi::geom::new_HandleGeomCurve_from_HandleGeom_BSplineCurve(&curve);
    let pnt = ffi::gp::new_point(x, y, z);
    let projection = ffi::geom_api::GeomAPI_ProjectPointOnCurve_new(&pnt, &curve).ok()?;
    if projection.NbPoints() < 1 {
        return None;
    }
    let value = projection.LowerDistanceParameter();
    value.is_finite().then_some(value)
}

/// A uniform clamped B-spline from control points (`Part.BSplineCurve(poles)`).
pub(crate) fn bspline_from_poles(
    coords: &[f64],
    degree: i32,
    periodic: bool,
) -> Option<UniquePtr<ffi::geom::Handle_Geom_BSplineCurve>> {
    let count = coords.len() / 3;
    if count < 2 || degree < 1 || periodic || count as i32 <= degree {
        return None;
    }
    let mut poles = ffi::t_col_gp::TColgp_Array1OfPnt_new(1, count as i32);
    for i in 0..count {
        let pnt = ffi::gp::new_point(coords[3 * i], coords[3 * i + 1], coords[3 * i + 2]);
        poles.pin_mut().SetValue(i as i32 + 1, &pnt);
    }
    let distinct = count as i32 - degree + 1;
    let mut knots = ffi::t_col_std::TColStd_Array1OfReal_new(1, distinct as i32);
    let mut mults = ffi::t_col_std::TColStd_Array1OfInteger_new(1, distinct as i32);
    for i in 1..=distinct {
        ffi::t_col_std::TColStd_Array1OfReal_SetValue(knots.pin_mut(), i as i32, (i - 1) as f64);
        ffi::t_col_std::TColStd_Array1OfInteger_SetValue(
            mults.pin_mut(),
            i as i32,
            if i == 1 || i == distinct { degree + 1 } else { 1 },
        );
    }
    ffi::geom::Geom_BSplineCurve_new(&poles, &knots, &mults, degree).ok()
}

/// A trimmed conic edge (3 = ellipse, 4 = hyperbola, 5 = parabola).
#[allow(clippy::too_many_arguments)]
pub(crate) fn conic_edge(
    kind: i32,
    center: Vector3,
    normal: Vector3,
    x_axis: Vector3,
    major: f64,
    minor: f64,
    first: f64,
    last: f64,
) -> Option<UniquePtr<TopoDS_Shape>> {
    let origin = ffi::gp::new_point(center.x, center.y, center.z);
    let main_dir = ffi::gp::gp_Dir_new(normal.x, normal.y, normal.z);
    let x_dir = ffi::gp::gp_Dir_new(x_axis.x, x_axis.y, x_axis.z);
    let axis = ffi::gp::gp_Ax2_new3(&origin, &main_dir, &x_dir).ok()?;
    let handle = match kind {
        3 => ffi::geom::new_HandleGeomCurve_from_HandleGeom_Ellipse(
            ffi::geom::Geom_Ellipse_new(&axis, major, minor).ok()?.as_ref().unwrap(),
        ),
        4 => ffi::geom::new_HandleGeomCurve_from_HandleGeom_Hyperbola(
            ffi::geom::Geom_Hyperbola_new(&axis, major, minor).ok()?.as_ref().unwrap(),
        ),
        5 => ffi::geom::new_HandleGeomCurve_from_HandleGeom_Parabola(
            ffi::geom::Geom_Parabola_new(&axis, major).ok()?.as_ref().unwrap(),
        ),
        _ => return None,
    };
    let mut make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeEdge_CurveRange(&handle, first, last).ok()?;
    if !make.IsDone() {
        return None;
    }
    Some(ffi::topo_ds::TopoDS_Shape_to_owned(
        ffi::topo_ds::cast_edge_to_shape(make.pin_mut().Edge()),
    ))
}

/// A full elliptical edge.
#[allow(clippy::too_many_arguments)]
pub(crate) fn ellipse_edge(
    center: Vector3,
    normal: Vector3,
    x_axis: Vector3,
    major: f64,
    minor: f64,
) -> Option<UniquePtr<TopoDS_Shape>> {
    let origin = ffi::gp::new_point(center.x, center.y, center.z);
    let main_dir = ffi::gp::gp_Dir_new(normal.x, normal.y, normal.z);
    let x_dir = ffi::gp::gp_Dir_new(x_axis.x, x_axis.y, x_axis.z);
    let axis = ffi::gp::gp_Ax2_new3(&origin, &main_dir, &x_dir).ok()?;
    let ellipse = ffi::geom::Geom_Ellipse_new(&axis, major, minor).ok()?;
    let handle = ffi::geom::new_HandleGeomCurve_from_HandleGeom_Ellipse(&ellipse);
    let mut make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeEdge_HandleGeomCurve(&handle);
    if !make.IsDone() {
        return None;
    }
    Some(ffi::topo_ds::TopoDS_Shape_to_owned(
        ffi::topo_ds::cast_edge_to_shape(make.pin_mut().Edge()),
    ))
}

/// A Bezier edge from explicit poles and weights.
pub(crate) fn bezier_edge(
    coords: &[f64],
    weights: &[f64],
) -> Option<UniquePtr<TopoDS_Shape>> {
    let count = coords.len() / 3;
    if count < 2 || weights.len() != count {
        return None;
    }
    let mut poles = ffi::t_col_gp::TColgp_Array1OfPnt_new(1, count as i32);
    let mut rational = false;
    for i in 0..count {
        let pnt = ffi::gp::new_point(coords[3 * i], coords[3 * i + 1], coords[3 * i + 2]);
        poles.pin_mut().SetValue(i as i32 + 1, &pnt);
        rational = rational || (weights[i] - 1.0).abs() > 1e-12;
    }
    let bezier = if rational {
        let mut values = ffi::t_col_std::TColStd_Array1OfReal_new(1, count as i32);
        for i in 0..count {
            ffi::t_col_std::TColStd_Array1OfReal_SetValue(
                values.pin_mut(),
                i as i32 + 1,
                weights[i],
            );
        }
        ffi::geom::Geom_BezierCurve_new_rational(&poles, &values).ok()?
    } else {
        ffi::geom::Geom_BezierCurve_new(&poles).ok()?
    };
    let handle = ffi::geom::new_HandleGeomCurve_from_HandleGeom_BezierCurve(&bezier);
    let mut make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeEdge_HandleGeomCurve(&handle);
    if !make.IsDone() {
        return None;
    }
    Some(ffi::topo_ds::TopoDS_Shape_to_owned(
        ffi::topo_ds::cast_edge_to_shape(make.pin_mut().Edge()),
    ))
}

/// A B-spline edge over `[first, last]`, re-segmenting when the range is narrower.
#[allow(clippy::too_many_arguments)]
pub(crate) fn bspline_edge_range(
    coords: &[f64],
    weights: &[f64],
    knots: &[f64],
    multiplicities: &[i32],
    degree: i32,
    periodic: bool,
    first: f64,
    last: f64,
) -> Option<UniquePtr<TopoDS_Shape>> {
    let mut curve = build(coords, weights, knots, multiplicities, degree, periodic, false)?;
    let domain_first = ffi::geom::HandleGeomCurve_FirstParameter(
        ffi::geom::new_HandleGeomCurve_from_HandleGeom_BSplineCurve(&curve)
            .as_ref()
            .unwrap(),
    );
    let domain_last = ffi::geom::HandleGeomCurve_LastParameter(
        ffi::geom::new_HandleGeomCurve_from_HandleGeom_BSplineCurve(&curve)
            .as_ref()
            .unwrap(),
    );
    if first > domain_first + 1e-12 || last < domain_last - 1e-12 {
        let mut segment = ffi::geom::HandleGeomBSplineCurve_Copy(&curve);
        if ffi::geom::HandleGeomBSplineCurve_Segment(segment.pin_mut(), first, last).is_err() {
            return None;
        }
        curve = segment;
    }
    let handle = ffi::geom::new_HandleGeomCurve_from_HandleGeom_BSplineCurve(&curve);
    let mut make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeEdge_HandleGeomCurve(&handle);
    if !make.IsDone() {
        return None;
    }
    Some(ffi::topo_ds::TopoDS_Shape_to_owned(
        ffi::topo_ds::cast_edge_to_shape(make.pin_mut().Edge()),
    ))
}

/// A flat xyz list.

// ---------------------------------------------------------------------------
// Analytic edge classification (`fc_edge_curve`'s typed replacement)
// ---------------------------------------------------------------------------

/// Read the analytic curve behind an edge, as a typed [`EdgeCurve`].
pub(crate) fn edge_curve(shape: &TopoDS_Shape) -> Result<EdgeCurve, GeomError> {
    if shape.IsNull() || shape.ShapeType() != TopAbs_ShapeEnum::TopAbs_EDGE {
        return Err(unsupported("curve_of: the shape is not an edge"));
    }
    let edge = TopoDS::Edge(shape);
    let mut first = 0.0;
    let mut last = 0.0;
    let curve = ffi::b_rep::BRep_Tool_Curve(edge, &mut first, &mut last);
    if curve.IsNull() {
        return Err(unsupported("curve_of: the edge has no curve"));
    }
    classify(curve, first, last, 0)
}

fn classify(
    input: UniquePtr<ffi::geom::Handle_Geom_Curve>,
    mut first: f64,
    mut last: f64,
    depth: i32,
) -> Result<EdgeCurve, GeomError> {
    if input.IsNull() || depth > 8 {
        return Err(unsupported("curve_of: unsupported curve type"));
    }
    let mut curve = input;
    if ffi::geom::GeomCurve_IsKind_trimmed(&curve) {
        let trimmed = ffi::geom::GeomCurve_DownCast_trimmed(&curve);
        if !trimmed.IsNull() {
            let upcast = ffi::geom::new_HandleGeomCurve_from_HandleGeom_TrimmedCurve(&trimmed);
            first = ffi::geom::HandleGeomCurve_FirstParameter(&upcast);
            last = ffi::geom::HandleGeomCurve_LastParameter(&upcast);
            curve = ffi::geom::Geom_TrimmedCurve_BasisCurve(&trimmed);
        }
    }
    if curve.IsNull() {
        return Err(unsupported("curve_of: unsupported curve type"));
    }

    if ffi::geom::GeomCurve_IsKind_line(&curve) {
        let line = ffi::geom::GeomCurve_DownCast_line(&curve);
        if line.IsNull() {
            return Err(unsupported("curve_of: unsupported line"));
        }
        let lin = ffi::geom::Geom_Line_Lin(&line);
        let origin = point(ffi::gp::gp_Lin_Location(&lin));
        let direction = direction(ffi::gp::gp_Lin_Direction(&lin));
        return Ok(EdgeCurve::Line(Line::new(origin, direction)?));
    }

    if ffi::geom::GeomCurve_IsKind_circle(&curve) {
        let circle = ffi::geom::GeomCurve_DownCast_circle(&curve);
        if circle.IsNull() {
            return Err(unsupported("curve_of: unsupported circle"));
        }
        let circ = ffi::geom::Geom_Circle_Circ(&circle);
        let position = ffi::gp::gp_Circ_Position(&circ);
        let origin = point(position.Location());
        let axis = direction(position.Direction());
        let x_axis = direction(position.XDirection());
        let radius = circ.Radius();
        let circle = Circle::with_axis(origin, axis, x_axis, radius)?;
        if (last - first).abs() >= std::f64::consts::TAU - 1e-9 {
            return Ok(EdgeCurve::Circle(circle));
        }
        return Ok(EdgeCurve::Arc(Arc::new(circle, first, last)?));
    }

    if ffi::geom::GeomCurve_IsKind_ellipse(&curve) {
        let ellipse = ffi::geom::GeomCurve_DownCast_ellipse(&curve);
        if ellipse.IsNull() {
            return Err(unsupported("curve_of: unsupported ellipse"));
        }
        let elips = ffi::geom::Geom_Ellipse_Elips(&ellipse);
        let position = ffi::gp::gp_Elips_Position(&elips);
        let origin = point(position.Location());
        let axis = direction(position.Direction());
        let x_axis = direction(position.XDirection());
        let ellipse = Ellipse::with_axis(
            origin,
            axis,
            x_axis,
            ffi::gp::gp_Elips_MajorRadius(&elips),
            ffi::gp::gp_Elips_MinorRadius(&elips),
        )?;
        if (last - first).abs() >= std::f64::consts::TAU - 1e-9 {
            return Ok(EdgeCurve::Ellipse(ellipse));
        }
        return Ok(EdgeCurve::EllipticalArc(EllipticalArc::new(
            ellipse, first, last,
        )?));
    }

    if ffi::geom::GeomCurve_IsKind_hyperbola(&curve) {
        let hyperbola = ffi::geom::GeomCurve_DownCast_hyperbola(&curve);
        if hyperbola.IsNull() {
            return Err(unsupported("curve_of: unsupported hyperbola"));
        }
        let hypr = ffi::geom::Geom_Hyperbola_Hypr(&hyperbola);
        let position = ffi::gp::gp_Hypr_Position(&hypr);
        let hyperbola = Hyperbola::with_axis(
            point(position.Location()),
            direction(position.Direction()),
            direction(position.XDirection()),
            ffi::gp::gp_Hypr_MajorRadius(&hypr),
            ffi::gp::gp_Hypr_MinorRadius(&hypr),
        )?;
        return Ok(EdgeCurve::ArcOfHyperbola(ArcOfHyperbola::new(
            hyperbola, first, last,
        )?));
    }

    if ffi::geom::GeomCurve_IsKind_parabola(&curve) {
        let parabola = ffi::geom::GeomCurve_DownCast_parabola(&curve);
        if parabola.IsNull() {
            return Err(unsupported("curve_of: unsupported parabola"));
        }
        let parab = ffi::geom::Geom_Parabola_Parab(&parabola);
        let position = ffi::gp::gp_Parab_Position(&parab);
        let parabola = Parabola::with_axis(
            point(position.Location()),
            direction(position.Direction()),
            direction(position.XDirection()),
            ffi::gp::gp_Parab_Focal(&parab),
        )?;
        return Ok(EdgeCurve::ArcOfParabola(ArcOfParabola::new(
            parabola, first, last,
        )?));
    }

    if ffi::geom::GeomCurve_IsKind_bezier(&curve) {
        let bezier = ffi::geom::GeomCurve_DownCast_bezier(&curve);
        if bezier.IsNull() {
            return Err(unsupported("curve_of: unsupported Bezier"));
        }
        let count = ffi::geom::HandleGeomBezierCurve_NbPoles(&bezier);
        let mut poles = Vec::with_capacity(count as usize);
        let mut weights = Vec::with_capacity(count as usize);
        for i in 1..=count {
            poles.push(point(&ffi::geom::HandleGeomBezierCurve_Pole(&bezier, i)));
            weights.push(ffi::geom::HandleGeomBezierCurve_Weight(&bezier, i));
        }
        let base = BezierCurve::new(poles, weights)?;
        let curve = if (first - 0.0).abs() <= 1e-12 && (last - 1.0).abs() <= 1e-12 {
            base
        } else {
            base.trim(first, last).unwrap_or(base)
        };
        return Ok(EdgeCurve::BezierCurve(curve));
    }

    if ffi::geom::GeomCurve_IsKind_bspline(&curve) {
        let spline = ffi::geom::GeomCurve_DownCast_bspline(&curve);
        if spline.IsNull() {
            return Err(unsupported("curve_of: unsupported B-spline"));
        }
        // A B-spline edge may trim a wider knot domain; re-segment a copy so the
        // bounds survive in the knot vector itself.
        let domain_first = ffi::geom::HandleGeomCurve_FirstParameter(
            ffi::geom::new_HandleGeomCurve_from_HandleGeom_BSplineCurve(&spline)
                .as_ref()
                .unwrap(),
        );
        let domain_last = ffi::geom::HandleGeomCurve_LastParameter(
            ffi::geom::new_HandleGeomCurve_from_HandleGeom_BSplineCurve(&spline)
                .as_ref()
                .unwrap(),
        );
        let spline = if first > domain_first + 1e-12 || last < domain_last - 1e-12 {
            let mut segment = ffi::geom::HandleGeomBSplineCurve_Copy(&spline);
            if ffi::geom::HandleGeomBSplineCurve_Segment(segment.pin_mut(), first, last).is_err() {
                return Err(unsupported("curve_of: unsupported B-spline range"));
            }
            first = ffi::geom::HandleGeomCurve_FirstParameter(
                ffi::geom::new_HandleGeomCurve_from_HandleGeom_BSplineCurve(&segment)
                    .as_ref()
                    .unwrap(),
            );
            last = ffi::geom::HandleGeomCurve_LastParameter(
                ffi::geom::new_HandleGeomCurve_from_HandleGeom_BSplineCurve(&segment)
                    .as_ref()
                    .unwrap(),
            );
            segment
        } else {
            spline
        };
        return bspline_edge_curve(&spline);
    }

    if ffi::geom::GeomCurve_IsKind_offset(&curve) {
        let offset = ffi::geom::GeomCurve_DownCast_offset(&curve);
        if offset.IsNull() {
            return Err(unsupported("curve_of: unsupported offset curve"));
        }
        let direction = direction(ffi::geom::Geom_OffsetCurve_Direction(&offset));
        let offset_value = ffi::geom::Geom_OffsetCurve_Offset(&offset);
        let basis = ffi::geom::Geom_OffsetCurve_BasisCurve(&offset);
        let basis_first = if basis.IsNull() {
            first
        } else {
            ffi::geom::HandleGeomCurve_FirstParameter(&basis)
        };
        let basis_last = if basis.IsNull() {
            last
        } else {
            ffi::geom::HandleGeomCurve_LastParameter(&basis)
        };
        let basis = classify(basis, basis_first, basis_last, depth + 1)?;
        return Ok(EdgeCurve::OffsetCurve(OffsetCurve::new(
            basis,
            offset_value,
            direction,
            first,
            last,
        )?));
    }

    Err(unsupported("curve_of: unsupported curve type"))
}

fn bspline_edge_curve(
    spline: &ffi::geom::Handle_Geom_BSplineCurve,
) -> Result<EdgeCurve, GeomError> {
    let degree = ffi::geom::HandleGeomBSplineCurve_Degree(spline);
    let pole_count = ffi::geom::HandleGeomBSplineCurve_NbPoles(spline);
    let knot_count = ffi::geom::HandleGeomBSplineCurve_NbKnots(spline);
    let periodic = ffi::geom::HandleGeomBSplineCurve_IsPeriodic(spline);

    let mut poles = Vec::with_capacity(pole_count as usize);
    let mut weights = Vec::with_capacity(pole_count as usize);
    for i in 1..=pole_count {
        poles.push(point(&ffi::geom::HandleGeomBSplineCurve_Pole(spline, i)));
        weights.push(ffi::geom::HandleGeomBSplineCurve_Weight(spline, i));
    }
    let mut knots = Vec::with_capacity(knot_count as usize);
    let mut mults = Vec::with_capacity(knot_count as usize);
    for i in 1..=knot_count {
        knots.push(ffi::geom::HandleGeomBSplineCurve_Knot(spline, i));
        mults.push(ffi::geom::HandleGeomBSplineCurve_Multiplicity(spline, i));
    }
    let curve = if periodic {
        NurbsCurve::new_periodic(degree, knots, mults, poles, weights)?
    } else {
        NurbsCurve::new(degree, knots, mults, poles, weights)?
    };
    Ok(EdgeCurve::BSplineCurve(curve))
}

// ---------------------------------------------------------------------------
// 2-D geometry (`fc_geom2d_surface`, `fc_bspline2d_edge`,
// `fc_curve2d_on_surface`, `fc_edge_pcurve`)
// ---------------------------------------------------------------------------

/// Build the surface described by `spec`, or fall back to `face`'s own surface,
/// or the world XY plane.
pub(crate) fn geom2d_surface(
    spec: Option<&SurfaceSpec>,
    face: &TopoDS_Shape,
) -> Option<UniquePtr<ffi::geom::Handle_Geom_Surface>> {
    if let Some(spec) = spec {
        let (origin, axis, x_dir, direct) = match spec {
            SurfaceSpec::Plane {
                origin,
                normal,
                x_dir,
                direct,
            } => (*origin, *normal, *x_dir, *direct),
            SurfaceSpec::Cylinder {
                origin,
                axis,
                x_dir,
                direct,
                ..
            } => (*origin, *axis, *x_dir, *direct),
            SurfaceSpec::Cone {
                origin,
                axis,
                x_dir,
                direct,
                ..
            } => (*origin, *axis, *x_dir, *direct),
            SurfaceSpec::Sphere {
                center,
                axis,
                x_dir,
                direct,
                ..
            } => (*center, *axis, *x_dir, *direct),
            SurfaceSpec::Torus {
                center,
                axis,
                x_dir,
                direct,
                ..
            } => (*center, *axis, *x_dir, *direct),
        };
        let origin = ffi::gp::new_point(origin.x, origin.y, origin.z);
        let axis = ffi::gp::gp_Dir_new(axis.x, axis.y, axis.z);
        let x_dir = ffi::gp::gp_Dir_new(x_dir.x, x_dir.y, x_dir.z);
        let mut frame = ffi::gp::gp_Ax3_new3(&origin, &axis, &x_dir).ok()?;
        if !direct {
            frame.pin_mut().YReverse();
        }
        let surface = match spec {
            SurfaceSpec::Plane { .. } => {
                let plane = ffi::gp::gp_Pln_new_frame(&frame).ok()?;
                let plane = ffi::geom::Geom_Plane_new(&plane);
                ffi::geom::plane_to_surface(&plane)
            }
            SurfaceSpec::Cylinder { radius, .. } => {
                let cylinder = ffi::geom::Geom_CylindricalSurface_new(&frame, *radius);
                ffi::geom::cylinder_to_surface(&cylinder)
            }
            SurfaceSpec::Cone {
                radius, semi_angle, ..
            } => {
                let cone = ffi::geom::Geom_ConicalSurface_new(&frame, *semi_angle, *radius).ok()?;
                ffi::geom::conical_to_surface(&cone)
            }
            SurfaceSpec::Sphere { radius, .. } => {
                let sphere = ffi::geom::Geom_SphericalSurface_new(&frame, *radius).ok()?;
                ffi::geom::spherical_to_surface(&sphere)
            }
            SurfaceSpec::Torus {
                major_radius,
                minor_radius,
                ..
            } => {
                let torus =
                    ffi::geom::Geom_ToroidalSurface_new(&frame, *major_radius, *minor_radius)
                        .ok()?;
                ffi::geom::toroidal_to_surface(&torus)
            }
        };
        return Some(surface);
    }
    if !face.IsNull() {
        return Some(ffi::b_rep::BRep_Tool_Surface(TopoDS::Face(face)));
    }
    let frame = ffi::gp::gp_Ax3_from_gp_Ax2(ffi::gp::gp::XOY());
    let plane = ffi::gp::gp_Pln_new_frame(&frame).ok()?;
    let plane = ffi::geom::Geom_Plane_new(&plane);
    Some(ffi::geom::plane_to_surface(&plane))
}

/// A 2-D B-spline lifted onto a surface as an edge.
#[allow(clippy::too_many_arguments)]
pub(crate) fn bspline2d_edge(
    coords: &[f64],
    weights: &[f64],
    knots: &[f64],
    multiplicities: &[i32],
    degree: i32,
    periodic: bool,
    surface: Option<&SurfaceSpec>,
    face: &TopoDS_Shape,
    first: f64,
    last: f64,
    has_range: bool,
) -> Option<UniquePtr<TopoDS_Shape>> {
    let pole_count = coords.len() / 2;
    let knot_count = knots.len();
    if pole_count < 2
        || knot_count < 1
        || degree < 1
        || degree > ffi::geom::HandleGeomBSplineCurve_MaxDegree()
        || weights.len() != pole_count
        || multiplicities.len() != knot_count
    {
        return None;
    }
    let mut poles = ffi::t_col_gp::TColgp_Array1OfPnt2d_new(1, pole_count as i32);
    let mut pole_weights = ffi::t_col_std::TColStd_Array1OfReal_new(1, pole_count as i32);
    for i in 0..pole_count {
        let pnt = ffi::gp::new_point_2d(coords[2 * i], coords[2 * i + 1]);
        poles.pin_mut().SetValue(i as i32 + 1, &pnt);
        ffi::t_col_std::TColStd_Array1OfReal_SetValue(
            pole_weights.pin_mut(),
            i as i32 + 1,
            weights[i],
        );
    }
    let mut knot_values = ffi::t_col_std::TColStd_Array1OfReal_new(1, knot_count as i32);
    let mut knot_mults = ffi::t_col_std::TColStd_Array1OfInteger_new(1, knot_count as i32);
    for i in 0..knot_count {
        ffi::t_col_std::TColStd_Array1OfReal_SetValue(
            knot_values.pin_mut(),
            i as i32 + 1,
            knots[i],
        );
        ffi::t_col_std::TColStd_Array1OfInteger_SetValue(
            knot_mults.pin_mut(),
            i as i32 + 1,
            multiplicities[i],
        );
    }
    // `Geom2d_BSplineCurve` full constructor carries weights and the periodic flag.
    let pcurve = ffi::geom2d::Geom2d_BSplineCurve_new_full(
        &poles,
        &pole_weights,
        &knot_values,
        &knot_mults,
        degree,
        periodic,
    )
    .ok()?;
    let pcurve = ffi::geom2d::new_HandleGeom2d_Curve_from_HandleGeom2d_BSplineCurve(&pcurve);
    let surface = geom2d_surface(surface, face)?;

    let mut make = if has_range {
        ffi::b_rep_builder_api::BRepBuilderAPI_MakeEdge_CurveSurface2dRange(
            &pcurve, &surface, first, last,
        )
        .ok()?
    } else {
        ffi::b_rep_builder_api::BRepBuilderAPI_MakeEdge_CurveSurface2d(&pcurve, &surface)
    };
    if !make.IsDone() {
        return None;
    }
    let edge = make.pin_mut().Edge();
    ffi::b_rep_lib::BRepLib_BuildCurves3d_full(
        ffi::topo_ds::cast_edge_to_shape(edge),
        ffi::precision::Precision::Confusion(),
        ffi::geom_api::GeomAbs_Shape::GeomAbs_C1,
        14,
        10000,
    )
    .ok()?;
    Some(ffi::topo_ds::TopoDS_Shape_to_owned(ffi::topo_ds::cast_edge_to_shape(edge)))
}

/// An analytic 2-D curve lifted onto a surface (`GeomProjLib::Curve2d`).
pub(crate) fn curve2d_on_surface(
    planar_edge: &TopoDS_Shape,
    surface: Option<&SurfaceSpec>,
    face: &TopoDS_Shape,
) -> Option<UniquePtr<TopoDS_Shape>> {
    if planar_edge.IsNull() || planar_edge.ShapeType() != TopAbs_ShapeEnum::TopAbs_EDGE {
        return None;
    }
    let edge = TopoDS::Edge(planar_edge);
    let mut location = ffi::top_loc::Location_new();
    let mut first = 0.0;
    let mut last = 0.0;
    let mut curve = ffi::b_rep::BRep_Tool_Curve_Location(
        edge,
        location.pin_mut(),
        &mut first,
        &mut last,
    );
    if curve.IsNull() || !(last > first) {
        return None;
    }
    if !location.IsIdentity() {
        let trsf = ffi::top_loc::TopLoc_Location_Transformation(&location);
        curve = ffi::geom::HandleGeomCurve_Transformed(&curve, &trsf).ok()?;
    }
    if curve.IsNull() {
        return None;
    }
    let surface = geom2d_surface(surface, face)?;

    let xy_frame = ffi::gp::gp_Ax3_from_gp_Ax2(ffi::gp::gp::XOY());
    let xy_plane = ffi::gp::gp_Pln_new_frame(&xy_frame).ok()?;
    let xy_plane = ffi::geom::Geom_Plane_new(&xy_plane);
    let xy_surface = ffi::geom::plane_to_surface(&xy_plane);
    let pcurve = ffi::geom_proj_lib::GeomProjLib_Curve2d(&curve, first, last, &xy_surface).ok()?;
    if pcurve.IsNull() {
        return None;
    }
    let mut make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeEdge_CurveSurface2dRange(
        &pcurve, &surface, first, last,
    )
    .ok()?;
    if !make.IsDone() {
        return None;
    }
    let edge = make.pin_mut().Edge();
    ffi::b_rep_lib::BRepLib_BuildCurves3d_full(
        ffi::topo_ds::cast_edge_to_shape(edge),
        ffi::precision::Precision::Confusion(),
        ffi::geom_api::GeomAbs_Shape::GeomAbs_C1,
        14,
        10000,
    )
    .ok()?;
    Some(ffi::topo_ds::TopoDS_Shape_to_owned(ffi::topo_ds::cast_edge_to_shape(edge)))
}

/// The pcurve an edge has on a face, as a B-spline state.
pub(crate) fn edge_pcurve(
    edge: &TopoDS_Shape,
    face: &TopoDS_Shape,
) -> Option<BsplineState> {
    let edge = TopoDS::Edge(edge);
    let face = TopoDS::Face(face);
    let mut first = 0.0;
    let mut last = 0.0;
    let pcurve = ffi::b_rep::BRep_Tool_CurveOnSurface_face(edge, face, &mut first, &mut last);
    if pcurve.IsNull() || !(last > first) {
        return None;
    }
    let bounded = ffi::geom2d::Geom2d_TrimmedCurve_new(&pcurve, first, last);
    let bounded = ffi::geom2d::HandleGeom2d_TrimmedCurve_to_curve(&bounded);
    let spline = ffi::geom2d_convert::Geom2dConvert_CurveToBSplineCurve(&bounded).ok()?;
    let spline = ffi::geom2d::Geom2dCurve_DownCast_bspline(&spline);
    if spline.IsNull() {
        return None;
    }

    let degree = ffi::geom2d::Geom2d_BSplineCurve_Degree(&spline);
    let periodic = ffi::geom2d::Geom2d_BSplineCurve_IsPeriodic(&spline);
    let pole_array = ffi::geom2d::Geom2d_BSplineCurve_Poles(&spline);
    let knot_array = ffi::geom2d::Geom2d_BSplineCurve_Knots(&spline);
    let mult_array = ffi::geom2d::Geom2d_BSplineCurve_Multiplicities(&spline);
    let pole_count = pole_array.Length();
    let knot_count = knot_array.Length();

    let mut lifted = Vec::with_capacity(pole_count as usize * 3);
    let mut weights = Vec::with_capacity(pole_count as usize);
    for i in 1..=pole_count {
        let pnt = ffi::t_col_gp::TColgp_Array1OfPnt2d_Value(&pole_array, i);
        lifted.push(pnt.X());
        lifted.push(pnt.Y());
        lifted.push(0.0);
        weights.push(1.0);
    }
    let mut knots = Vec::with_capacity(knot_count as usize);
    let mut mults = Vec::with_capacity(knot_count as usize);
    for i in 1..=knot_count {
        knots.push(ffi::t_col_std::TColStd_Array1OfReal_Value(&knot_array, i));
        mults.push(ffi::t_col_std::TColStd_Array1OfInteger_Value(&mult_array, i));
    }

    // Re-read the state through the 3-D constructor so the layout and the
    // periodicity handling stay in one place (`build`).
    let curve = build(&lifted, &weights, &knots, &mults, degree, periodic, false)?;
    state(&curve)
}

// ---------------------------------------------------------------------------
// Local surface/curve properties
// ---------------------------------------------------------------------------

/// The surface normal at `(u, v)` (`BRepLProp_SLProps` through the CSLib
/// fallback), with the face orientation applied.
pub(crate) fn face_normal(face: &TopoDS_Face, u: f64, v: f64) -> Option<Vector3> {
    let surface = ffi::b_rep_adaptor::BRepAdaptor_Surface_new(face);
    let mut props = ffi::b_rep_l_prop::BRepLProp_SLProps_new(
        &surface,
        u,
        v,
        2,
        ffi::precision::Precision::Confusion(),
    )
    .ok()?;
    let mut direction = normal_by_slprop(
        props.pin_mut(),
        u,
        v,
        surface.LastUParameter(),
        surface.LastVParameter(),
    )?;
    if face.Orientation() == ffi::top_abs::TopAbs_Orientation::TopAbs_REVERSED {
        direction = ffi::gp::gp_Dir_Reversed(&direction);
    }
    Some(direction_of(&direction))
}

/// The two unit surface tangents at `(u, v)`.
pub(crate) fn face_tangents(face: &TopoDS_Face, u: f64, v: f64) -> Option<(Vector3, Vector3)> {
    let surface = ffi::b_rep_adaptor::BRepAdaptor_Surface_new(face);
    let mut props = ffi::b_rep_l_prop::BRepLProp_SLProps_new(
        &surface,
        u,
        v,
        2,
        ffi::precision::Precision::Confusion(),
    )
    .ok()?;
    if !props.pin_mut().IsTangentUDefined() || !props.pin_mut().IsTangentVDefined() {
        return None;
    }
    let du = ffi::b_rep_l_prop::SLProps_TangentU(props.pin_mut()).ok()?;
    let dv = ffi::b_rep_l_prop::SLProps_TangentV(props.pin_mut()).ok()?;
    Some((direction_of(&du), direction_of(&dv)))
}

/// The two partial derivatives of order 1 or 2 at `(u, v)`.
pub(crate) fn face_derivatives(
    face: &TopoDS_Face,
    u: f64,
    v: f64,
    order: i32,
) -> Option<(Vector3, Vector3)> {
    let surface = ffi::b_rep_adaptor::BRepAdaptor_Surface_new(face);
    let mut props = ffi::b_rep_l_prop::BRepLProp_SLProps_new(
        &surface,
        u,
        v,
        order.max(1),
        ffi::precision::Precision::Confusion(),
    )
    .ok()?;
    let (du, dv) = if order >= 2 {
        (
            ffi::b_rep_l_prop::SLProps_D2U(props.pin_mut()).ok()?,
            ffi::b_rep_l_prop::SLProps_D2V(props.pin_mut()).ok()?,
        )
    } else {
        (
            ffi::b_rep_l_prop::SLProps_D1U(props.pin_mut()).ok()?,
            ffi::b_rep_l_prop::SLProps_D1V(props.pin_mut()).ok()?,
        )
    };
    Some((vector_of(&du), vector_of(&dv)))
}

/// The `(min, max)` principal curvatures at `(u, v)`.
pub(crate) fn face_curvature(face: &TopoDS_Face, u: f64, v: f64) -> Option<(f64, f64)> {
    let surface = ffi::b_rep_adaptor::BRepAdaptor_Surface_new(face);
    let mut props = ffi::b_rep_l_prop::BRepLProp_SLProps_new(
        &surface,
        u,
        v,
        2,
        ffi::precision::Precision::Confusion(),
    )
    .ok()?;
    if !props.pin_mut().IsCurvatureDefined() {
        return None;
    }
    Some((
        ffi::b_rep_l_prop::SLProps_MinCurvature(props.pin_mut()).ok()?,
        ffi::b_rep_l_prop::SLProps_MaxCurvature(props.pin_mut()).ok()?,
    ))
}

/// `Geom_Surface::Bounds` of the surface behind a face (untrimmed).
pub(crate) fn face_surface_bounds(face: &TopoDS_Face) -> Option<(f64, f64, f64, f64)> {
    let surface = ffi::b_rep::BRep_Tool_Surface(face);
    if surface.IsNull() {
        return None;
    }
    let (mut u1, mut u2, mut v1, mut v2) = (0.0, 0.0, 0.0, 0.0);
    ffi::geom::Geom_Surface_Bounds(&surface, &mut u1, &mut u2, &mut v1, &mut v2);
    Some((u1, u2, v1, v2))
}

/// The principal normal of a curve at `parameter`.
pub(crate) fn edge_normal(edge: &TopoDS_Edge, parameter: f64) -> Option<Vector3> {
    let curve = ffi::b_rep_adaptor::BRepAdaptor_Curve_new(edge);
    let mut props = ffi::b_rep_l_prop::BRepLProp_CLProps_new(
        &curve,
        parameter,
        2,
        ffi::precision::Precision::Confusion(),
    )
    .ok()?;
    let normal = ffi::b_rep_l_prop::CLProps_Normal(props.pin_mut()).ok()?;
    Some(direction_of(&normal))
}

/// The curve derivative of order 1, 2 or 3 at `parameter`.
pub(crate) fn edge_derivatives(edge: &TopoDS_Edge, parameter: f64, order: i32) -> Option<Vector3> {
    let curve = ffi::b_rep_adaptor::BRepAdaptor_Curve_new(edge);
    let mut props = ffi::b_rep_l_prop::BRepLProp_CLProps_new(
        &curve,
        parameter,
        order.max(1),
        ffi::precision::Precision::Confusion(),
    )
    .ok()?;
    let derivative = if order >= 3 {
        ffi::b_rep_l_prop::CLProps_D3(props.pin_mut()).ok()?
    } else if order == 2 {
        ffi::b_rep_l_prop::CLProps_D2(props.pin_mut()).ok()?
    } else {
        ffi::b_rep_l_prop::CLProps_D1(props.pin_mut()).ok()?
    };
    Some(vector_of(&derivative))
}

/// The curvature at `parameter`.
pub(crate) fn edge_curvature(edge: &TopoDS_Edge, parameter: f64) -> Option<f64> {
    let curve = ffi::b_rep_adaptor::BRepAdaptor_Curve_new(edge);
    let mut props = ffi::b_rep_l_prop::BRepLProp_CLProps_new(
        &curve,
        parameter,
        2,
        ffi::precision::Precision::Confusion(),
    )
    .ok()?;
    if !props.pin_mut().IsTangentDefined() {
        return None;
    }
    let value = ffi::b_rep_l_prop::CLProps_Curvature(props.pin_mut()).ok()?;
    value.is_finite().then_some(value)
}

fn vector_of(vec: &ffi::gp::gp_Vec) -> Vector3 {
    Vector3::new(vec.X(), vec.Y(), vec.Z())
}

fn direction_of(dir: &ffi::gp::gp_Dir) -> Vector3 {
    Vector3::new(dir.X(), dir.Y(), dir.Z())
}

/// `Part::Tools::getNormalBySLProp`: the SLProps normal with the `CSLib` fallback
/// at a degenerate or boundary point.
fn normal_by_slprop(
    mut props: std::pin::Pin<&mut ffi::b_rep_l_prop::BRepLProp_SLProps>,
    u: f64,
    v: f64,
    last_u: f64,
    last_v: f64,
) -> Option<UniquePtr<ffi::gp::gp_Dir>> {
    let tolerance = ffi::precision::Precision::Confusion();
    let d1u = ffi::b_rep_l_prop::SLProps_D1U(props.as_mut()).ok()?;
    let d1v = ffi::b_rep_l_prop::SLProps_D1V(props.as_mut()).ok()?;
    if d1u.Magnitude() > tolerance
        && d1v.Magnitude() > tolerance
        && props.as_mut().IsNormalDefined()
    {
        return ffi::b_rep_l_prop::SLProps_Normal(props.as_mut()).ok();
    }
    let d2u = ffi::b_rep_l_prop::SLProps_D2U(props.as_mut()).ok()?;
    let d2v = ffi::b_rep_l_prop::SLProps_D2V(props.as_mut()).ok()?;
    let duv = ffi::b_rep_l_prop::SLProps_DUV(props.as_mut()).ok()?;
    let mut status = ffi::cs_lib::CSLib_NormalStatus::CSLib_Singular;
    let mut direction = ffi::cs_lib::CSLib_Normal(&d1u, &d1v, &d2u, &d2v, &duv, tolerance, &mut status)
        .ok()?;
    match status {
        ffi::cs_lib::CSLib_NormalStatus::CSLib_D1NuIsNull => {
            if (last_v - v).abs() < tolerance {
                direction = ffi::gp::gp_Dir_Reversed(&direction);
            }
        }
        ffi::cs_lib::CSLib_NormalStatus::CSLib_D1NvIsNull
        | ffi::cs_lib::CSLib_NormalStatus::CSLib_D1NuIsParallelD1Nv => {
            if (last_u - u).abs() < tolerance {
                direction = ffi::gp::gp_Dir_Reversed(&direction);
            }
        }
        _ => {}
    }
    Some(direction)
}

/// `TopExp::FirstVertex`/`LastVertex` with orientation applied.
pub(crate) fn edge_first_vertex(shape: &TopoDS_Shape) -> Option<UniquePtr<TopoDS_Shape>> {
    let vertex = ffi::top_exp::TopExp_FirstVertex_Oriented(TopoDS::Edge(shape), true);
    if ffi::topo_ds::cast_vertex_to_shape(&vertex).IsNull() {
        return None;
    }
    Some(ffi::topo_ds::TopoDS_Shape_to_owned(
        ffi::topo_ds::cast_vertex_to_shape(&vertex),
    ))
}

pub(crate) fn edge_last_vertex(shape: &TopoDS_Shape) -> Option<UniquePtr<TopoDS_Shape>> {
    let vertex = ffi::top_exp::TopExp_LastVertex_Oriented(TopoDS::Edge(shape), true);
    if ffi::topo_ds::cast_vertex_to_shape(&vertex).IsNull() {
        return None;
    }
    Some(ffi::topo_ds::TopoDS_Shape_to_owned(
        ffi::topo_ds::cast_vertex_to_shape(&vertex),
    ))
}

pub(crate) fn wire_first_vertex(shape: &TopoDS_Shape) -> Option<UniquePtr<TopoDS_Shape>> {
    let mut first = ffi::topo_ds::TopoDS_Vertex_new();
    let mut last = ffi::topo_ds::TopoDS_Vertex_new();
    ffi::top_exp::TopExp::TopExp_WireVertices(
        TopoDS::Wire(shape),
        first.pin_mut(),
        last.pin_mut(),
    );
    if ffi::topo_ds::cast_vertex_to_shape(&first).IsNull() {
        return None;
    }
    Some(ffi::topo_ds::TopoDS_Shape_to_owned(
        ffi::topo_ds::cast_vertex_to_shape(&first),
    ))
}

pub(crate) fn wire_last_vertex(shape: &TopoDS_Shape) -> Option<UniquePtr<TopoDS_Shape>> {
    let mut first = ffi::topo_ds::TopoDS_Vertex_new();
    let mut last = ffi::topo_ds::TopoDS_Vertex_new();
    ffi::top_exp::TopExp::TopExp_WireVertices(
        TopoDS::Wire(shape),
        first.pin_mut(),
        last.pin_mut(),
    );
    if ffi::topo_ds::cast_vertex_to_shape(&last).IsNull() {
        return None;
    }
    Some(ffi::topo_ds::TopoDS_Shape_to_owned(
        ffi::topo_ds::cast_vertex_to_shape(&last),
    ))
}
