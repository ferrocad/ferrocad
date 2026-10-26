#pragma once

// FerroCAD: the Part shape operations `opencascade-sys` 0.3 does not bridge.
//
// Companion to `include/fc_topology.hxx` and `include/fc_curve.hxx`: sewing,
// NURBS conversion, edge sampling, shell free edges, elementary-surface faces
// and B-spline surface interpolation. Kept under `src/` so it sits beside the
// bridge that consumes it; every function is `inline`, so cxx emits it into the
// generated shim and no separate translation unit is needed.
//
// Nothing may cross the bridge except plain data or a shape handle: every entry
// point catches OCCT exceptions and reports failure with `nullptr`, an empty
// vector or `NaN`.

#include <BRepAdaptor_Curve.hxx>
#include <BRepAdaptor_Surface.hxx>
#include <BRepBuilderAPI_Copy.hxx>
#include <BRepBuilderAPI_MakeFace.hxx>
#include <BRepBuilderAPI_NurbsConvert.hxx>
#include <BRepBuilderAPI_Sewing.hxx>
#include <BRepLib.hxx>
#include <BRep_Builder.hxx>
#include <BRep_Tool.hxx>
#include <GCPnts_AbscissaPoint.hxx>
#include <GCPnts_TangentialDeflection.hxx>
#include <GCPnts_UniformAbscissa.hxx>
#include <GCPnts_UniformDeflection.hxx>
#include <GeomAPI_PointsToBSplineSurface.hxx>
#include <Geom2d_Curve.hxx>
#include <GeomProjLib.hxx>
#include <Geom_BSplineSurface.hxx>
#include <Geom_TrimmedCurve.hxx>
#include <Geom_ConicalSurface.hxx>
#include <Geom_CylindricalSurface.hxx>
#include <Geom_Plane.hxx>
#include <Geom_SphericalSurface.hxx>
#include <Geom_ToroidalSurface.hxx>
#include <Precision.hxx>
#include <ShapeAnalysis_Shell.hxx>
#include <TColgp_Array2OfPnt.hxx>
#include <TColStd_Array1OfReal.hxx>
#include <TopAbs_Orientation.hxx>
#include <TopAbs_ShapeEnum.hxx>
#include <TopExp.hxx>
#include <TopExp_Explorer.hxx>
#include <TopTools_IndexedMapOfShape.hxx>
#include <TopTools_ListIteratorOfListOfShape.hxx>
#include <TopTools_ListOfShape.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Compound.hxx>
#include <TopoDS_Face.hxx>
#include <TopoDS_Shape.hxx>
#include <TopoDS_Wire.hxx>
#include <gp_Ax3.hxx>
#include <gp_Dir.hxx>
#include <gp_Pnt.hxx>

#include <cmath>
#include <limits>
#include <memory>
#include <stdexcept>

#include "rust/cxx.h"

// --- sewing ----------------------------------------------------------------

// The face's backing surface frame, tagged by kind:
//
//   1  plane     origin xyz, x-direction xyz, y-direction xyz, normal xyz  (13)
//   2  cylinder  origin xyz, axis xyz, x-direction xyz, radius             (11)
//   3  sphere    centre xyz, axis xyz, x-direction xyz, radius             (11)
//   4  cone      origin xyz, axis xyz, x-direction xyz, radius, semi-angle (12)
//   5  torus     centre xyz, axis xyz, x-direction xyz, major, minor       (12)
//
// This is `fc_face_surface_data` with the cone and torus cases added; any other
// surface leaves `out` empty and the caller keeps the type id instead.
inline void fc_face_surface_frame(const TopoDS_Shape& shape, rust::Vec<double>& out)
{
    try {
        BRepAdaptor_Surface surface(TopoDS::Face(shape));
        const auto push_point = [&out](const gp_Pnt& p) {
            out.push_back(p.X());
            out.push_back(p.Y());
            out.push_back(p.Z());
        };
        const auto push_dir = [&out](const gp_Dir& d) {
            out.push_back(d.X());
            out.push_back(d.Y());
            out.push_back(d.Z());
        };

        switch (surface.GetType()) {
            case GeomAbs_Plane: {
                const gp_Ax3 pos = surface.Plane().Position();
                out.push_back(1.0);
                push_point(pos.Location());
                push_dir(pos.XDirection());
                push_dir(pos.YDirection());
                push_dir(pos.Direction());
                out.push_back(pos.Direct() ? 1.0 : 0.0);
                break;
            }
            case GeomAbs_Cylinder: {
                const gp_Cylinder cylinder = surface.Cylinder();
                const gp_Ax3 pos = cylinder.Position();
                out.push_back(2.0);
                push_point(pos.Location());
                push_dir(pos.Direction());
                push_dir(pos.XDirection());
                out.push_back(cylinder.Radius());
                out.push_back(pos.Direct() ? 1.0 : 0.0);
                break;
            }
            case GeomAbs_Sphere: {
                const gp_Sphere sphere = surface.Sphere();
                const gp_Ax3 pos = sphere.Position();
                out.push_back(3.0);
                push_point(pos.Location());
                push_dir(pos.Direction());
                push_dir(pos.XDirection());
                out.push_back(sphere.Radius());
                out.push_back(pos.Direct() ? 1.0 : 0.0);
                break;
            }
            case GeomAbs_Cone: {
                const gp_Cone cone = surface.Cone();
                const gp_Ax3 pos = cone.Position();
                out.push_back(4.0);
                push_point(pos.Location());
                push_dir(pos.Direction());
                push_dir(pos.XDirection());
                out.push_back(cone.RefRadius());
                out.push_back(cone.SemiAngle());
                out.push_back(pos.Direct() ? 1.0 : 0.0);
                break;
            }
            case GeomAbs_Torus: {
                const gp_Torus torus = surface.Torus();
                const gp_Ax3 pos = torus.Position();
                out.push_back(5.0);
                push_point(pos.Location());
                push_dir(pos.Direction());
                push_dir(pos.XDirection());
                out.push_back(torus.MajorRadius());
                out.push_back(torus.MinorRadius());
                out.push_back(pos.Direct() ? 1.0 : 0.0);
                break;
            }
            default:
                break;
        }
    }
    catch (...) {
        out.clear();
    }
}

// --- sewing ----------------------------------------------------------------

// `TopoShape::sewShape`: join the boundaries of `shape`'s faces into shells.
// Null on failure.
inline std::unique_ptr<TopoDS_Shape> fc_sew_shape(const TopoDS_Shape& shape,
                                                  double tolerance)
{
    if (shape.IsNull() || !std::isfinite(tolerance) || tolerance <= 0.0)
        return nullptr;
    try {
        BRepBuilderAPI_Sewing sew(tolerance);
        sew.Load(shape);
        sew.Perform();
        const TopoDS_Shape sewn = sew.SewedShape();
        if (sewn.IsNull())
            return nullptr;
        return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(sewn));
    }
    catch (...) {
        return nullptr;
    }
}

// --- NURBS conversion ------------------------------------------------------

// `TopoShape::toNurbs`: rewrite every curve and surface as a B-spline.
inline std::unique_ptr<TopoDS_Shape> fc_to_nurbs(const TopoDS_Shape& shape)
{
    if (shape.IsNull())
        return nullptr;
    try {
        BRepBuilderAPI_NurbsConvert convert(shape);
        const TopoDS_Shape result = convert.Shape();
        if (result.IsNull())
            return nullptr;
        return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(result));
    }
    catch (...) {
        return nullptr;
    }
}

// --- edge sampling ---------------------------------------------------------

// `Edge.discretize`. `mode` is 0 = Number, 1 = Distance, 2 = Deflection,
// 3 = Angular/Curvature. Returns flat xyz triples; empty on failure.
inline rust::Vec<double> fc_edge_discretize(const TopoDS_Shape& shape,
                                            int mode,
                                            double value,
                                            double curvature,
                                            int minimum,
                                            double first,
                                            double last)
{
    rust::Vec<double> out;
    if (shape.IsNull() || shape.ShapeType() != TopAbs_EDGE)
        return out;
    try {
        BRepAdaptor_Curve adapt(TopoDS::Edge(shape));
        const double first_parameter =
            std::isfinite(first) ? first : adapt.FirstParameter();
        const double last_parameter =
            std::isfinite(last) ? last : adapt.LastParameter();
        if (!(first_parameter < last_parameter))
            return out;

        const auto push = [&out](const gp_Pnt& p) {
            out.push_back(p.X());
            out.push_back(p.Y());
            out.push_back(p.Z());
        };

        switch (mode) {
            case 0: {
                if (value < 2.0)
                    return out;
                GCPnts_UniformAbscissa discretizer;
                discretizer.Initialize(
                    adapt, static_cast<int>(value), first_parameter, last_parameter);
                if (!discretizer.IsDone() || discretizer.NbPoints() <= 0)
                    return out;
                for (int i = 1; i <= discretizer.NbPoints(); ++i)
                    push(adapt.Value(discretizer.Parameter(i)));
                break;
            }
            case 1: {
                if (!(value > 0.0))
                    return out;
                GCPnts_UniformAbscissa discretizer;
                discretizer.Initialize(adapt, value, first_parameter, last_parameter);
                if (!discretizer.IsDone() || discretizer.NbPoints() <= 0)
                    return out;
                for (int i = 1; i <= discretizer.NbPoints(); ++i)
                    push(adapt.Value(discretizer.Parameter(i)));
                break;
            }
            case 2: {
                if (!(value > 0.0))
                    return out;
                GCPnts_UniformDeflection discretizer(
                    adapt, value, first_parameter, last_parameter);
                if (!discretizer.IsDone() || discretizer.NbPoints() <= 0)
                    return out;
                for (int i = 1; i <= discretizer.NbPoints(); ++i)
                    push(discretizer.Value(i));
                break;
            }
            case 3: {
                if (!(value > 0.0) || !(curvature >= 0.0))
                    return out;
                GCPnts_TangentialDeflection discretizer(
                    adapt,
                    first_parameter,
                    last_parameter,
                    value,
                    curvature,
                    minimum > 0 ? minimum : 2);
                if (discretizer.NbPoints() <= 0)
                    return out;
                for (int i = 1; i <= discretizer.NbPoints(); ++i)
                    push(discretizer.Value(i));
                break;
            }
            default:
                return out;
        }
    }
    catch (...) {
        out.clear();
    }
    return out;
}

// `Edge.getParameterByLength`: the curve parameter `length` along the edge.
// `NaN` when the range is infinite or the length is out of range.
inline double fc_edge_parameter_by_length(const TopoDS_Shape& shape,
                                          double length,
                                          double tolerance)
{
    if (shape.IsNull() || shape.ShapeType() != TopAbs_EDGE || !std::isfinite(length))
        return std::numeric_limits<double>::quiet_NaN();
    try {
        BRepAdaptor_Curve adapt(TopoDS::Edge(shape));
        const double first = adapt.FirstParameter();
        const double last = adapt.LastParameter();
        if (Precision::IsInfinite(first) || Precision::IsInfinite(last))
            return length;
        const double total = GCPnts_AbscissaPoint::Length(
            adapt, tolerance > 0.0 ? tolerance : Precision::Confusion());
        if (length < -total || length > total)
            return std::numeric_limits<double>::quiet_NaN();
        if (length < 0.0)
            length = total + length;
        GCPnts_AbscissaPoint point(
            tolerance > 0.0 ? tolerance : Precision::Confusion(), adapt, length, first);
        return point.Parameter();
    }
    catch (...) {
        return std::numeric_limits<double>::quiet_NaN();
    }
}

// --- shell free edges ------------------------------------------------------

// `Shell.getFreeEdges`: the boundary edges of a shell, as a compound.
// `ShapeAnalysis_Shell` reports every edge that is not shared by two faces.
inline std::unique_ptr<TopoDS_Shape> fc_shell_free_edges(const TopoDS_Shape& shell)
{
    if (shell.IsNull())
        return nullptr;
    try {
        ShapeAnalysis_Shell analysis;
        analysis.LoadShells(shell);
        analysis.CheckOrientedShells(shell, Standard_True, Standard_True);
        const TopoDS_Compound compound = analysis.FreeEdges();
        if (compound.IsNull())
            return nullptr;
        return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(compound));
    }
    catch (...) {
        return nullptr;
    }
}

// --- faces on an elementary surface ---------------------------------------

// Build the pcurves of a wire's edges on `surface` and return a copy of the
// wire that carries them.
//
// The wire a caller gets from a face carries pcurves on *that face's* surface
// handle. A fresh `Geom_Surface` built from an elementary description is a
// different handle, so `BRepBuilderAPI_MakeFace(surface, wire)` would see no 2D
// representation and produce a zero-area face. Copying the wire first keeps the
// caller's shape untouched, and projecting each edge onto the new surface gives
// the new handle the pcurves it needs.
inline TopoDS_Wire fc_wire_on_surface(const TopoDS_Wire& wire,
                                      const Handle(Geom_Surface)& surface)
{
    BRepBuilderAPI_Copy copier(wire, Standard_True, Standard_False);
    if (!copier.IsDone() || copier.Shape().ShapeType() != TopAbs_WIRE)
        return TopoDS_Wire();
    TopoDS_Wire copy = TopoDS::Wire(copier.Shape());
    // The pcurves below are computed from the globally located 3D curves, so the
    // copy is read in the global frame too.
    copy.Location(TopLoc_Location());
    BRep_Builder builder;
    for (TopExp_Explorer it(copy, TopAbs_EDGE); it.More(); it.Next()) {
        const TopoDS_Edge edge = TopoDS::Edge(it.Current());
        Standard_Real first = 0.0;
        Standard_Real last = 0.0;
        const Handle(Geom_Curve) curve = BRep_Tool::Curve(edge, first, last);
        if (curve.IsNull())
            continue;
        // Project the trimmed piece, not the basis curve: projecting a full
        // circle and then rescaling its range would take the long way round.
        const Handle(Geom_Curve) piece = new Geom_TrimmedCurve(curve, first, last);
        const Handle(Geom2d_Curve) pcurve =
            GeomProjLib::Curve2d(piece, first, last, surface);
        if (pcurve.IsNull())
            return TopoDS_Wire();
        builder.UpdateEdge(edge, pcurve, surface, TopLoc_Location(), Precision::Confusion());
        BRepLib::SameRange(edge, Precision::Confusion());
    }
    return copy;
}

// `Part.Face(surface, wires)`. `spec` is the flat description written by
// `ferrocad_occt::bridge::surface_spec`:
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
// The first wire bounds the face and the rest are added as further boundaries,
// exactly as upstream's `BRepBuilderAPI_MakeFace(Geom_Surface, wire)` followed by
// `Add(wire)` does (no reordering or reversing).
inline std::unique_ptr<TopoDS_Shape> fc_make_face_on_surface(
    rust::Slice<const double> spec,
    const TopTools_ListOfShape& wires)
{
    if (spec.size() < 15 || wires.IsEmpty())
        return nullptr;
    try {
        const gp_Pnt origin(spec[1], spec[2], spec[3]);
        const gp_Dir axis(spec[4], spec[5], spec[6]);
        const gp_Dir x_direction(spec[7], spec[8], spec[9]);
        gp_Ax3 frame(origin, axis, x_direction);
        if (spec[14] == 0.0)
            frame.YReverse();

        Handle(Geom_Surface) surface;
        switch (static_cast<int>(spec[0])) {
            case 0:
                surface = new Geom_Plane(frame);
                break;
            case 1:
                surface = new Geom_CylindricalSurface(frame, spec[10]);
                break;
            case 2:
                surface = new Geom_ConicalSurface(frame, spec[11], spec[10]);
                break;
            case 3:
                surface = new Geom_SphericalSurface(frame, spec[10]);
                break;
            case 4:
                surface = new Geom_ToroidalSurface(frame, spec[12], spec[13]);
                break;
            default:
                return nullptr;
        }
        if (surface.IsNull())
            return nullptr;

        TopTools_ListIteratorOfListOfShape it(wires);
        if (!it.More())
            return nullptr;
        if (it.Value().ShapeType() != TopAbs_WIRE)
            return nullptr;
        const TopoDS_Wire outer = fc_wire_on_surface(TopoDS::Wire(it.Value()), surface);
        if (outer.IsNull())
            return nullptr;
        BRepBuilderAPI_MakeFace make(surface, outer, Standard_True);
        if (!make.IsDone())
            return nullptr;
        for (it.Next(); it.More(); it.Next()) {
            if (it.Value().ShapeType() != TopAbs_WIRE)
                return nullptr;
            const TopoDS_Wire inner = fc_wire_on_surface(TopoDS::Wire(it.Value()), surface);
            if (inner.IsNull())
                return nullptr;
            make.Add(inner);
            if (!make.IsDone())
                return nullptr;
        }
        if (make.Face().IsNull())
            return nullptr;
        return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(make.Face()));
    }
    catch (...) {
        return nullptr;
    }
}

// --- B-spline surface interpolation ---------------------------------------

// `BSplineSurface.interpolate`: `GeomAPI_PointsToBSplineSurface` through a
// row-major `nx` by `ny` grid of points (flat xyz triples, `rows[u][v]`).
//
// Returns the surface's parameter bounds and expanded knot sequences:
//
//   0      u first, 1 u last, 2 v first, 3 v last
//   4      nu, then nu u knots
//   4+nu+1 nv, then nv v knots
//
// Empty on failure.
inline rust::Vec<double> fc_bspline_surface_interpolate(rust::Slice<const double> coords,
                                                        int nx,
                                                        int ny)
{
    rust::Vec<double> out;
    if (nx < 2 || ny < 2 || coords.size() != static_cast<std::size_t>(3 * nx * ny))
        return out;
    try {
        TColgp_Array2OfPnt points(1, nx, 1, ny);
        for (int u = 0; u < nx; ++u) {
            for (int v = 0; v < ny; ++v) {
                const std::size_t at = 3 * (static_cast<std::size_t>(u) * ny + v);
                points.SetValue(
                    u + 1, v + 1, gp_Pnt(coords[at], coords[at + 1], coords[at + 2]));
            }
        }
        GeomAPI_PointsToBSplineSurface interpolation;
        interpolation.Interpolate(points);
        const Handle(Geom_BSplineSurface) surface = interpolation.Surface();
        if (surface.IsNull())
            return out;

        out.push_back(surface->UKnot(1));
        out.push_back(surface->UKnot(surface->NbUKnots()));
        out.push_back(surface->VKnot(1));
        out.push_back(surface->VKnot(surface->NbVKnots()));

        Standard_Integer u_length = 0;
        for (Standard_Integer i = 1; i <= surface->NbUKnots(); ++i)
            u_length += surface->UMultiplicity(i);
        TColStd_Array1OfReal u_sequence(1, u_length);
        surface->UKnotSequence(u_sequence);
        out.push_back(u_length);
        for (Standard_Integer i = 1; i <= u_length; ++i)
            out.push_back(u_sequence.Value(i));

        Standard_Integer v_length = 0;
        for (Standard_Integer i = 1; i <= surface->NbVKnots(); ++i)
            v_length += surface->VMultiplicity(i);
        TColStd_Array1OfReal v_sequence(1, v_length);
        surface->VKnotSequence(v_sequence);
        out.push_back(v_length);
        for (Standard_Integer i = 1; i <= v_length; ++i)
            out.push_back(v_sequence.Value(i));
    }
    catch (...) {
        out.clear();
    }
    return out;
}

// The face of the interpolating B-spline surface through the grid
// (`BSplineSurface.toShape`). Null on failure.
inline std::unique_ptr<TopoDS_Shape> fc_bspline_surface_face(rust::Slice<const double> coords,
                                                             int nx,
                                                             int ny)
{
    if (nx < 2 || ny < 2 || coords.size() != static_cast<std::size_t>(3 * nx * ny))
        return nullptr;
    try {
        TColgp_Array2OfPnt points(1, nx, 1, ny);
        for (int u = 0; u < nx; ++u) {
            for (int v = 0; v < ny; ++v) {
                const std::size_t at = 3 * (static_cast<std::size_t>(u) * ny + v);
                points.SetValue(
                    u + 1, v + 1, gp_Pnt(coords[at], coords[at + 1], coords[at + 2]));
            }
        }
        GeomAPI_PointsToBSplineSurface interpolation;
        interpolation.Interpolate(points);
        const Handle(Geom_BSplineSurface) surface = interpolation.Surface();
        if (surface.IsNull())
            return nullptr;
        BRepBuilderAPI_MakeFace make(surface, Precision::Confusion());
        if (!make.IsDone())
            return nullptr;
        return std::make_unique<TopoDS_Shape>(make.Shape());
    }
    catch (...) {
        return nullptr;
    }
}

// A copy of `shape` with `orientation` (`TopAbs_FORWARD`=0, REVERSED=1,
// INTERNAL=2, EXTERNAL=3). Null on failure.
inline std::unique_ptr<TopoDS_Shape> fc_with_orientation(const TopoDS_Shape& shape,
                                                         int32_t orientation)
{
    if (shape.IsNull() || orientation < 0 || orientation > 3)
        return nullptr;
    try {
        TopoDS_Shape copy = shape;
        copy.Orientation(static_cast<TopAbs_Orientation>(orientation));
        return std::make_unique<TopoDS_Shape>(copy);
    }
    catch (...) {
        return nullptr;
    }
}

// --- aggregate enumeration -------------------------------------------------

// The sub-shapes of `shape` whose type is `type` (`TopExp::MapShapes`), as a
// compound. Used by `Shape.Compounds`/`Shape.CompSolids`; the caller reads the
// compound's direct children. A shape whose own type matches is included, which
// is what `TopExp::MapShapes` does.
inline std::unique_ptr<TopoDS_Shape> fc_shape_sub_shapes_of_type(const TopoDS_Shape& shape,
                                                                 int32_t type)
{
    if (shape.IsNull())
        return nullptr;
    try {
        TopTools_IndexedMapOfShape map;
        TopExp::MapShapes(shape, static_cast<TopAbs_ShapeEnum>(type), map);
        TopoDS_Compound compound;
        BRep_Builder builder;
        builder.MakeCompound(compound);
        for (int i = 1; i <= map.Extent(); ++i)
            builder.Add(compound, map.FindKey(i));
        return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(compound));
    }
    catch (...) {
        return nullptr;
    }
}
