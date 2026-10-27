#pragma once

// FerroCAD: the 2-D half of the kernel bridge.
//
// Three things live here, all of them seams `opencascade-sys` 0.3 does not
// bridge:
//
//  - a `Geom2d_BSplineCurve` lifted onto a `Geom_Surface` (`BRepBuilderAPI_MakeEdge`
//    with the pcurve/surface form, i.e. upstream's `Geometry2dPy::toShape`), which
//    is what `Part.Geom2d.*.toShape(surface)` builds;
//  - the pcurve an edge has on a face (`BRep_Tool::CurveOnSurface`), which
//    `Part.Face.curveOnSurface(edge)` reports;
//  - local surface/curve properties that involve a *non-planar* surface
//    (`BRepLProp_SLProps` / `BRepLProp_CLProps`), which `Face.normalAt`,
//    `Face.tangentAt`, `Face.derivative1At`/`derivative2At`, `Face.curvatureAt`
//    and `Edge.normalAt` need. The planar shortcuts the Rust side used before
//    cannot answer those.
//
// Included after `fc_curve.hxx`, which supplies `fc_bspline_curve` and
// `fc_bspline_push_state`.

#include <BRepAdaptor_Curve.hxx>
#include <BRepAdaptor_Surface.hxx>
#include <BRepBuilderAPI_MakeEdge.hxx>
#include <BRepLib.hxx>
#include <BRepLProp_CLProps.hxx>
#include <BRepLProp_SLProps.hxx>
#include <BRep_Tool.hxx>
#include <CSLib.hxx>
#include <CSLib_NormalStatus.hxx>
#include <Geom2dConvert.hxx>
#include <Geom2d_TrimmedCurve.hxx>
#include <Geom2d_BSplineCurve.hxx>
#include <Geom2d_Curve.hxx>
#include <GeomAPI_PointsToBSpline.hxx>
#include <GeomAPI_ProjectPointOnCurve.hxx>
#include <Geom_ConicalSurface.hxx>
#include <Geom_CylindricalSurface.hxx>
#include <Geom_Plane.hxx>
#include <Geom_SphericalSurface.hxx>
#include <Geom_Surface.hxx>
#include <Geom_ToroidalSurface.hxx>
#include <Precision.hxx>
#include <TColgp_Array1OfPnt2d.hxx>
#include <TopExp.hxx>
#include <TopoDS_Vertex.hxx>
#include <gp.hxx>
#include <gp_Ax3.hxx>
#include <gp_Pln.hxx>

#include <algorithm>
#include <cmath>
#include <limits>

// --- the surface a pcurve is lifted onto ------------------------------------

// The surface described by `spec`, in the layout
// `ferrocad_occt::backend::surface_spec` writes (the same one
// `fc_make_face_on_surface` takes):
//
//   0      kind (0 plane, 1 cylinder, 2 cone, 3 sphere, 4 torus)
//   1..4   origin (plane/cylinder/cone) or centre (sphere/torus)
//   4..7   axis (the plane normal for a plane)
//   7..10  the surface's `u = 0` direction
//   10     radius (cylinder, cone, sphere)
//   11     semi-angle (cone)
//   12..13 major and minor radius (torus)
//   14     whether the frame is direct (1) or indirect (0)
//
// An empty `spec` falls back to `face`'s own surface (this is how a B-spline or
// any other face-carried surface is handed in), and with neither the surface is
// the world XY plane — upstream's `toShape()` without a surface argument.
inline Handle(Geom_Surface) fc_geom2d_surface(rust::Slice<const double> spec,
                                              const TopoDS_Shape& face)
{
    if (spec.size() >= 15) {
        try {
            const gp_Pnt origin(spec[1], spec[2], spec[3]);
            const gp_Dir axis(spec[4], spec[5], spec[6]);
            const gp_Dir x_direction(spec[7], spec[8], spec[9]);
            gp_Ax3 frame(origin, axis, x_direction);
            if (spec[14] == 0.0)
                frame.YReverse();
            switch (static_cast<int>(spec[0])) {
                case 0:
                    return new Geom_Plane(frame);
                case 1:
                    return new Geom_CylindricalSurface(frame, spec[10]);
                case 2:
                    return new Geom_ConicalSurface(frame, spec[11], spec[10]);
                case 3:
                    return new Geom_SphericalSurface(frame, spec[10]);
                case 4:
                    return new Geom_ToroidalSurface(frame, spec[12], spec[13]);
                default:
                    return nullptr;
            }
        }
        catch (...) {
            return nullptr;
        }
    }
    if (!face.IsNull()) {
        try {
            return BRep_Tool::Surface(TopoDS::Face(face));
        }
        catch (...) {
            return nullptr;
        }
    }
    return new Geom_Plane(gp::XOY());
}

// --- a 2-D B-spline as an edge on a surface ---------------------------------

// `coords` is a flat xy list, one weight per pole, one multiplicity per knot.
// The curve is the pcurve; the returned edge carries the surface image as its
// 3-D curve, which is what `BRepLib::BuildCurves3d` computes (upstream's
// `create3dCurve` for the non-analytic case). `has_range` trims the pcurve to
// `[first, last]`. Returns null on failure.
inline std::unique_ptr<TopoDS_Shape> fc_bspline2d_edge(
    rust::Slice<const double> coords,
    rust::Slice<const double> weights,
    rust::Slice<const double> knots,
    rust::Slice<const int32_t> multiplicities,
    int degree,
    bool periodic,
    rust::Slice<const double> surface_spec,
    const TopoDS_Shape& surface_face,
    double first,
    double last,
    bool has_range)
{
    const int pole_count = static_cast<int>(coords.size() / 2);
    const int knot_count = static_cast<int>(knots.size());
    if (pole_count < 2 || knot_count < 1 || degree < 1
        || degree > Geom_BSplineCurve::MaxDegree()
        || static_cast<int>(weights.size()) != pole_count
        || static_cast<int>(multiplicities.size()) != knot_count) {
        return nullptr;
    }
    try {
        TColgp_Array1OfPnt2d poles(1, pole_count);
        TColStd_Array1OfReal pole_weights(1, pole_count);
        for (int i = 0; i < pole_count; ++i) {
            poles.SetValue(i + 1, gp_Pnt2d(coords[2 * i], coords[2 * i + 1]));
            pole_weights.SetValue(i + 1, weights[i]);
        }
        TColStd_Array1OfReal knot_values(1, knot_count);
        TColStd_Array1OfInteger knot_mults(1, knot_count);
        for (int i = 0; i < knot_count; ++i) {
            knot_values.SetValue(i + 1, knots[i]);
            knot_mults.SetValue(i + 1, multiplicities[i]);
        }
        const Handle(Geom2d_BSplineCurve) pcurve =
            new Geom2d_BSplineCurve(poles, pole_weights, knot_values, knot_mults, degree, periodic);

        const Handle(Geom_Surface) surface = fc_geom2d_surface(surface_spec, surface_face);
        if (surface.IsNull())
            return nullptr;

        TopoDS_Edge edge;
        if (has_range) {
            BRepBuilderAPI_MakeEdge make(pcurve, surface, first, last);
            if (!make.IsDone())
                return nullptr;
            edge = make.Edge();
        }
        else {
            BRepBuilderAPI_MakeEdge make(pcurve, surface);
            if (!make.IsDone())
                return nullptr;
            edge = make.Edge();
        }
        // Build the 3-D curve from the pcurve, as upstream's `create3dCurve`
        // does for a B-spline pcurve.
        BRepLib::BuildCurves3d(edge, Precision::Confusion(), GeomAbs_C1, 14, 10000);
        return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(edge));
    }
    catch (...) {
        return nullptr;
    }
}

// --- the pcurve of an edge on a face ----------------------------------------

// `BRep_Tool::CurveOnSurface`, converted to a B-spline and reported in the state
// layout `fc_bspline_push_state` writes (so the poles are `(x, y, 0)`). The
// state's parameter range is the edge's own `[first, last]`, because the pcurve
// shares the edge's parameterisation. Empty when the edge has no pcurve on that
// face.
//
// The basis is trimmed to `[first, last]` before the conversion: the pcurve of a
// straight generator is an *unbounded* `Geom2d_Line`, and
// `Geom2dConvert::CurveToBSplineCurve` only accepts bounded curves.
inline rust::Vec<double> fc_edge_pcurve(const TopoDS_Shape& edge_shape,
                                        const TopoDS_Shape& face_shape)
{
    rust::Vec<double> out;
    try {
        const TopoDS_Edge edge = TopoDS::Edge(edge_shape);
        const TopoDS_Face face = TopoDS::Face(face_shape);
        Standard_Real first = 0.0;
        Standard_Real last = 0.0;
        const Handle(Geom2d_Curve) pcurve = BRep_Tool::CurveOnSurface(edge, face, first, last);
        if (pcurve.IsNull())
            return out;
        if (!(last > first))
            return out;
        const Handle(Geom2d_Curve) bounded = new Geom2d_TrimmedCurve(pcurve, first, last);
        const Handle(Geom2d_BSplineCurve) spline = Geom2dConvert::CurveToBSplineCurve(bounded);
        if (spline.IsNull())
            return out;

        const TColgp_Array1OfPnt2d& poles = spline->Poles();
        rust::Vec<double> lifted;
        lifted.reserve(static_cast<size_t>(spline->NbPoles()) * 3);
        for (Standard_Integer i = poles.Lower(); i <= poles.Upper(); ++i) {
            lifted.push_back(poles.Value(i).X());
            lifted.push_back(poles.Value(i).Y());
            lifted.push_back(0.0);
        }
        TColStd_Array1OfReal weight_values(1, spline->NbPoles());
        spline->Weights(weight_values);
        rust::Vec<double> weights;
        for (Standard_Integer i = weight_values.Lower(); i <= weight_values.Upper(); ++i) {
            weights.push_back(weight_values.Value(i));
        }
        const TColStd_Array1OfReal& knots = spline->Knots();
        rust::Vec<double> knot_values;
        for (Standard_Integer i = knots.Lower(); i <= knots.Upper(); ++i) {
            knot_values.push_back(knots.Value(i));
        }
        const TColStd_Array1OfInteger& mults = spline->Multiplicities();
        rust::Vec<int32_t> multiplicities;
        for (Standard_Integer i = mults.Lower(); i <= mults.Upper(); ++i) {
            multiplicities.push_back(static_cast<int32_t>(mults.Value(i)));
        }
        // Re-read the state through the 3-D constructor so the layout and the
        // periodicity handling stay in one place (`fc_bspline_curve`).
        const Handle(Geom_BSplineCurve) lifted_spline =
            fc_bspline_curve(rust::Slice<const double>(lifted),
                             rust::Slice<const double>(weights),
                             rust::Slice<const double>(knot_values),
                             rust::Slice<const int32_t>(multiplicities),
                             spline->Degree(),
                             spline->IsPeriodic(),
                             false);
        if (lifted_spline.IsNull())
            return out;
        fc_bspline_push_state(out, lifted_spline);
    }
    catch (...) {
        out.clear();
    }
    return out;
}

// --- local properties of a surface ------------------------------------------

// `Geom_Surface::Bounds` of the surface behind a face (the **untrimmed**
// parameter range, unlike the face's own trimmed range). Empty on failure.
inline rust::Vec<double> fc_face_surface_bounds(const TopoDS_Shape& shape)
{
    rust::Vec<double> out;
    try {
        const Handle(Geom_Surface) surface = BRep_Tool::Surface(TopoDS::Face(shape));
        if (surface.IsNull())
            return out;
        Standard_Real u1 = 0.0;
        Standard_Real u2 = 0.0;
        Standard_Real v1 = 0.0;
        Standard_Real v2 = 0.0;
        surface->Bounds(u1, u2, v1, v2);
        out.push_back(u1);
        out.push_back(u2);
        out.push_back(v1);
        out.push_back(v2);
    }
    catch (...) {
        out.clear();
    }
    return out;
}

// Upstream's `Part::Tools::getNormalBySLProp`: the SLProps normal, with the
// `CSLib` fallback at a degenerate or boundary point. `last_u`/`last_v` are the
// surface's upper bounds, used to flip the fallback normal at the boundary.
template <typename Prop>
inline void fc_geom2d_normal_by_slprop(Prop& prop,
                                       double u,
                                       double v,
                                       double last_u,
                                       double last_v,
                                       double tolerance,
                                       gp_Dir& direction,
                                       Standard_Boolean& done)
{
    if (prop.D1U().Magnitude() > tolerance && prop.D1V().Magnitude() > tolerance
        && prop.IsNormalDefined()) {
        direction = prop.Normal();
        done = Standard_True;
        return;
    }
    CSLib_NormalStatus status = CSLib_Singular;
    CSLib::Normal(
        prop.D1U(), prop.D1V(), prop.D2U(), prop.D2V(), prop.DUV(), tolerance, done, status, direction);
    // At the right boundary the normal is flipped with respect to the normals on
    // the surrounding points.
    if (status == CSLib_D1NuIsNull) {
        if (std::abs(last_v - v) < tolerance)
            direction.Reverse();
    }
    else if (status == CSLib_D1NvIsNull || status == CSLib_D1NuIsParallelD1Nv) {
        if (std::abs(last_u - u) < tolerance)
            direction.Reverse();
    }
}

// `Face.normalAt(u, v)`: the surface normal at `(u, v)`, flipped when the face's
// orientation is reversed (upstream's `Tools::getNormal(const TopoDS_Face&)`).
// Empty when the normal is not defined there.
inline rust::Vec<double> fc_face_normal(const TopoDS_Shape& shape, double u, double v)
{
    rust::Vec<double> out;
    try {
        const TopoDS_Face face = TopoDS::Face(shape);
        BRepAdaptor_Surface surface(face);
        BRepLProp_SLProps props(surface, u, v, 2, Precision::Confusion());
        gp_Dir direction;
        Standard_Boolean done = Standard_False;
        fc_geom2d_normal_by_slprop(props,
                                   u,
                                   v,
                                   surface.LastUParameter(),
                                   surface.LastVParameter(),
                                   Precision::Confusion(),
                                   direction,
                                   done);
        if (!done)
            return out;
        if (face.Orientation() == TopAbs_REVERSED)
            direction.Reverse();
        out.push_back(direction.X());
        out.push_back(direction.Y());
        out.push_back(direction.Z());
    }
    catch (...) {
        out.clear();
    }
    return out;
}

// `Face.tangentAt(u, v)`: the two unit tangents (`BRepLProp_SLProps::TangentU/V`).
inline rust::Vec<double> fc_face_tangents(const TopoDS_Shape& shape, double u, double v)
{
    rust::Vec<double> out;
    try {
        BRepAdaptor_Surface surface(TopoDS::Face(shape));
        BRepLProp_SLProps props(surface, u, v, 2, Precision::Confusion());
        if (!props.IsTangentUDefined() || !props.IsTangentVDefined())
            return out;
        gp_Dir du;
        gp_Dir dv;
        props.TangentU(du);
        props.TangentV(dv);
        out.push_back(du.X());
        out.push_back(du.Y());
        out.push_back(du.Z());
        out.push_back(dv.X());
        out.push_back(dv.Y());
        out.push_back(dv.Z());
    }
    catch (...) {
        out.clear();
    }
    return out;
}

// `Face.derivative1At` / `derivative2At`: the two partial derivatives of the
// given order (1 or 2), in `(du, dv)` pairs.
inline rust::Vec<double> fc_face_derivatives(const TopoDS_Shape& shape,
                                            double u,
                                            double v,
                                            int order)
{
    rust::Vec<double> out;
    try {
        BRepAdaptor_Surface surface(TopoDS::Face(shape));
        BRepLProp_SLProps props(surface, u, v, std::max(order, 1), Precision::Confusion());
        const gp_Vec du = order >= 2 ? props.D2U() : props.D1U();
        const gp_Vec dv = order >= 2 ? props.D2V() : props.D1V();
        out.push_back(du.X());
        out.push_back(du.Y());
        out.push_back(du.Z());
        out.push_back(dv.X());
        out.push_back(dv.Y());
        out.push_back(dv.Z());
    }
    catch (...) {
        out.clear();
    }
    return out;
}

// `Face.curvatureAt(u, v)`: `(min, max)` principal curvatures.
inline rust::Vec<double> fc_face_curvature(const TopoDS_Shape& shape, double u, double v)
{
    rust::Vec<double> out;
    try {
        BRepAdaptor_Surface surface(TopoDS::Face(shape));
        BRepLProp_SLProps props(surface, u, v, 2, Precision::Confusion());
        if (!props.IsCurvatureDefined())
            return out;
        out.push_back(props.MinCurvature());
        out.push_back(props.MaxCurvature());
    }
    catch (...) {
        out.clear();
    }
    return out;
}

// --- local properties of a curve -------------------------------------------

// `Edge.normalAt(param)`: the principal normal (`BRepLProp_CLProps::Normal`).
inline rust::Vec<double> fc_edge_normal(const TopoDS_Shape& shape, double parameter)
{
    rust::Vec<double> out;
    try {
        BRepAdaptor_Curve curve(TopoDS::Edge(shape));
        BRepLProp_CLProps props(curve, parameter, 2, Precision::Confusion());
        gp_Dir direction;
        props.Normal(direction);
        out.push_back(direction.X());
        out.push_back(direction.Y());
        out.push_back(direction.Z());
    }
    catch (...) {
        out.clear();
    }
    return out;
}

// `Edge.derivative1At` / `derivative2At` / `derivative3At`.
inline rust::Vec<double> fc_edge_derivatives(const TopoDS_Shape& shape,
                                            double parameter,
                                            int order)
{
    rust::Vec<double> out;
    try {
        BRepAdaptor_Curve curve(TopoDS::Edge(shape));
        BRepLProp_CLProps props(curve, parameter, std::max(order, 1), Precision::Confusion());
        const gp_Vec derivative = order >= 3 ? props.D3() : (order == 2 ? props.D2() : props.D1());
        out.push_back(derivative.X());
        out.push_back(derivative.Y());
        out.push_back(derivative.Z());
    }
    catch (...) {
        out.clear();
    }
    return out;
}

// `Edge.curvatureAt(param)`; NaN when the curvature is not defined there.
inline double fc_edge_curvature(const TopoDS_Shape& shape, double parameter)
{
    try {
        BRepAdaptor_Curve curve(TopoDS::Edge(shape));
        BRepLProp_CLProps props(curve, parameter, 2, Precision::Confusion());
        if (!props.IsTangentDefined())
            return std::numeric_limits<double>::quiet_NaN();
        return props.Curvature();
    }
    catch (...) {
        return std::numeric_limits<double>::quiet_NaN();
    }
}

// --- knot insertion ---------------------------------------------------------

// `Geom_BSplineCurve::RemoveKnot`: the state of the same curve with knot `index`
// reduced to multiplicity `mult`, or empty when the removal is rejected (the
// curve would have to change by more than `tolerance`).
inline rust::Vec<double> fc_bspline_remove_knot(rust::Slice<const double> coords,
                                               rust::Slice<const double> weights,
                                               rust::Slice<const double> knots,
                                               rust::Slice<const int32_t> multiplicities,
                                               int degree,
                                               bool periodic,
                                               bool check_rational,
                                               int index,
                                               int mult,
                                               double tolerance)
{
    rust::Vec<double> out;
    try {
        Handle(Geom_BSplineCurve) curve = fc_bspline_curve(
            coords, weights, knots, multiplicities, degree, periodic, check_rational);
        if (curve.IsNull())
            return out;
        if (!curve->RemoveKnot(index, mult, tolerance))
            return out;
        fc_bspline_push_state(out, curve);
    }
    catch (...) {
        out.clear();
    }
    return out;
}

// `TopExp::FirstVertex`/`LastVertex` with the orientation applied: the endpoint
// an edge starts at (and ends at) in its own direction. A `Reversed` edge starts
// at the vertex the curve ends on, which is exactly the distinction Draft's
// `get_extended_wire` relies on. Null when the edge has no such vertex.
inline std::unique_ptr<TopoDS_Shape> fc_edge_first_vertex(const TopoDS_Shape& shape)
{
    try {
        const TopoDS_Vertex vertex = TopExp::FirstVertex(TopoDS::Edge(shape), Standard_True);
        if (vertex.IsNull())
            return nullptr;
        return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(vertex));
    }
    catch (...) {
        return nullptr;
    }
}

inline std::unique_ptr<TopoDS_Shape> fc_edge_last_vertex(const TopoDS_Shape& shape)
{
    try {
        const TopoDS_Vertex vertex = TopExp::LastVertex(TopoDS::Edge(shape), Standard_True);
        if (vertex.IsNull())
            return nullptr;
        return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(vertex));
    }
    catch (...) {
        return nullptr;
    }
}

// `TopExp::Vertices(wire, V1, V2)`: a wire's *stored* first and last vertex, the
// pair upstream's `TopoShapeWirePy::getOrderedVertexes` uses to decide whether an
// open wire needs its final vertex appended. For a `Reversed` wire this is not
// the traversal order, which is exactly why upstream uses it here.
inline std::unique_ptr<TopoDS_Shape> fc_wire_first_vertex(const TopoDS_Shape& shape)
{
    try {
        TopoDS_Vertex first;
        TopoDS_Vertex last;
        TopExp::Vertices(TopoDS::Wire(shape), first, last);
        if (first.IsNull())
            return nullptr;
        return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(first));
    }
    catch (...) {
        return nullptr;
    }
}

inline std::unique_ptr<TopoDS_Shape> fc_wire_last_vertex(const TopoDS_Shape& shape)
{
    try {
        TopoDS_Vertex first;
        TopoDS_Vertex last;
        TopExp::Vertices(TopoDS::Wire(shape), first, last);
        if (last.IsNull())
            return nullptr;
        return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(last));
    }
    catch (...) {
        return nullptr;
    }
}

// `GeomCurve.parameter(point)`: the curve parameter of the closest point
// (`GeomAPI_ProjectPointOnCurve`). NaN when the projection fails.
inline double fc_bspline_parameter(rust::Slice<const double> coords,
                                   rust::Slice<const double> weights,
                                   rust::Slice<const double> knots,
                                   rust::Slice<const int32_t> multiplicities,
                                   int degree,
                                   bool periodic,
                                   bool check_rational,
                                   double x,
                                   double y,
                                   double z)
{
    try {
        Handle(Geom_BSplineCurve) curve = fc_bspline_curve(
            coords, weights, knots, multiplicities, degree, periodic, check_rational);
        if (curve.IsNull())
            return std::numeric_limits<double>::quiet_NaN();
        GeomAPI_ProjectPointOnCurve projection(gp_Pnt(x, y, z), curve);
        if (projection.NbPoints() < 1)
            return std::numeric_limits<double>::quiet_NaN();
        return projection.LowerDistanceParameter();
    }
    catch (...) {
        return std::numeric_limits<double>::quiet_NaN();
    }
}

// `GeomAPI_PointsToBSpline` (`BSplineCurve.approximate`), reported in the state
// layout `fc_bspline_push_state` writes. `coords` is a flat xyz list; the degree
// is bounded by `degree_min`/`degree_max` and the `C2` continuity tolerance by
// `tolerance`. Empty when the approximation fails.
inline rust::Vec<double> fc_bspline_approximated(rust::Slice<const double> coords,
                                                int degree_min,
                                                int degree_max,
                                                double tolerance)
{
    rust::Vec<double> out;
    try {
        const int count = static_cast<int>(coords.size() / 3);
        if (count < 2)
            return out;
        TColgp_Array1OfPnt points(1, count);
        for (int i = 0; i < count; ++i) {
            points.SetValue(i + 1, gp_Pnt(coords[3 * i], coords[3 * i + 1], coords[3 * i + 2]));
        }
        GeomAPI_PointsToBSpline approximation(points, degree_min, degree_max, GeomAbs_C2, tolerance);
        if (!approximation.IsDone())
            return out;
        fc_bspline_push_state(out, approximation.Curve());
    }
    catch (...) {
        out.clear();
    }
    return out;
}

// `Geom_BSplineCurve::InsertKnot`: the state of the same curve with `u` inserted
// up to multiplicity `mult`. Empty when the arguments do not describe a valid
// curve or the insertion is rejected (an out-of-range parameter, or a
// multiplicity the curve cannot take).
inline rust::Vec<double> fc_bspline_insert_knot(rust::Slice<const double> coords,
                                               rust::Slice<const double> weights,
                                               rust::Slice<const double> knots,
                                               rust::Slice<const int32_t> multiplicities,
                                               int degree,
                                               bool periodic,
                                               bool check_rational,
                                               double u,
                                               int mult,
                                               double tolerance)
{
    rust::Vec<double> out;
    try {
        Handle(Geom_BSplineCurve) curve = fc_bspline_curve(
            coords, weights, knots, multiplicities, degree, periodic, check_rational);
        if (curve.IsNull() || mult < 1)
            return out;
        curve->InsertKnot(u, mult, tolerance);
        fc_bspline_push_state(out, curve);
    }
    catch (...) {
        out.clear();
    }
    return out;
}
