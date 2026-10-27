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

#include <BRepAdaptor_CompCurve.hxx>
#include <BRepAdaptor_Curve.hxx>
#include <BRepAdaptor_Surface.hxx>
#include <BRepAlgoAPI_Defeaturing.hxx>
#include <BRepAlgo_Image.hxx>
#include <BRepBndLib.hxx>
#include <BRepBuilderAPI_Copy.hxx>
#include <BRepBuilderAPI_MakeFace.hxx>
#include <BRepBuilderAPI_MakeSolid.hxx>
#include <BRepBuilderAPI_NurbsConvert.hxx>
#include <BRepBuilderAPI_Sewing.hxx>
#include <BRepCheck_Analyzer.hxx>
#include <BRepClass_FaceClassifier.hxx>
#include <BRepLib.hxx>
#include <BRepLib_FindSurface.hxx>
#include <BRepOffsetAPI_MakeOffsetShape.hxx>
#include <BRepOffsetAPI_ThruSections.hxx>
#include <BRepOffset_Mode.hxx>
#include <BRepProj_Projection.hxx>
#include <BRepTools.hxx>
#include <BRepTools_WireExplorer.hxx>
#include <BRepTopAdaptor_FClass2d.hxx>
#include <BRep_Builder.hxx>
#include <BRep_Tool.hxx>
#include <GCPnts_AbscissaPoint.hxx>
#include <GCPnts_TangentialDeflection.hxx>
#include <GCPnts_UniformAbscissa.hxx>
#include <GCPnts_UniformDeflection.hxx>
#include <GeomAPI_PointsToBSplineSurface.hxx>
#include <GeomAPI_ProjectPointOnSurf.hxx>
#include <Geom2d_Curve.hxx>
#include <GeomAbs_JoinType.hxx>
#include <GeomProjLib.hxx>
#include <Geom_BSplineSurface.hxx>
#include <Geom_TrimmedCurve.hxx>
#include <Geom_ConicalSurface.hxx>
#include <Geom_CylindricalSurface.hxx>
#include <Geom_Plane.hxx>
#include <Geom_SphericalSurface.hxx>
#include <Geom_ToroidalSurface.hxx>
#include <Precision.hxx>
#include <ShapeAnalysis_FreeBoundsProperties.hxx>
#include <ShapeAnalysis_Shell.hxx>
#include <ShapeAnalysis_Surface.hxx>
#include <ShapeFix_Shape.hxx>
#include <ShapeFix_Wire.hxx>
#include <ShapeUpgrade_ShellSewing.hxx>
#include <TColgp_Array2OfPnt.hxx>
#include <TColStd_Array1OfInteger.hxx>
#include <TColStd_Array1OfReal.hxx>
#include <TColStd_Array2OfReal.hxx>
#include <Bnd_Box.hxx>
#include <TopAbs_State.hxx>
#include <TopLoc_Location.hxx>
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
#include <TopoDS_Iterator.hxx>
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

// `ShapeUpgrade_ShellSewing::ApplySewing`: the re-sew `Part.makeShell` falls
// back to when the naive `BRep_Builder` assembly is not valid (upstream's
// `TopoShape::makeElementBoolean` with `OpCodes::Shell`). Unlike
// `fc_sew_shape`'s single `BRepBuilderAPI_Sewing` pass, this runs the
// upgrading sewing (and the face fixers it needs) to the default tolerance, so
// independent triangle/plane faces that merely share vertices become one
// closed shell. Null on failure.
inline std::unique_ptr<TopoDS_Shape> fc_sew_shell(const TopoDS_Shape& shape)
{
    if (shape.IsNull())
        return nullptr;
    try {
        ShapeUpgrade_ShellSewing sewing;
        const TopoDS_Shape sewn = sewing.ApplySewing(shape);
        if (sewn.IsNull())
            return nullptr;
        return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(sewn));
    }
    catch (...) {
        return nullptr;
    }
}

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

// `Wire.discretize`, upstream's `TopoShapeWirePy::discretize`. The same modes
// and layout as `fc_edge_discretize`, over `BRepAdaptor_CompCurve` so the samples
// follow the wire's accumulated parameterisation across its edges.
inline rust::Vec<double> fc_wire_discretize(const TopoDS_Shape& shape,
                                            int mode,
                                            double value,
                                            double curvature,
                                            int minimum,
                                            double first,
                                            double last)
{
    rust::Vec<double> out;
    if (shape.IsNull() || shape.ShapeType() != TopAbs_WIRE)
        return out;
    try {
        BRepAdaptor_CompCurve adapt(TopoDS::Wire(shape));
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

// Build the pcurves of a wire's edges on `surface` and return the wire to hand to
// `BRepBuilderAPI_MakeFace`.
//
// The wire a caller gets from a face carries pcurves on *that face's* surface
// handle. A fresh `Geom_Surface` built from an elementary description is a
// different handle, so `BRepBuilderAPI_MakeFace(surface, wire)` would see no 2D
// representation and produce a zero-area face.
//
// The edges are **updated in place** (an OCCT edge may carry pcurves for several
// surfaces), not copied: the new face then shares its edges with the faces around
// it, so a rebuilt face still sews into the original shell. Copying the wire here
// would make `Part.makeShell([...rebuilt...])` fail to close, because the rebuilt
// face's edges would no longer be the neighbours' edges.
//
// A wire with a non-identity location is copied and flattened to the global frame
// instead: the projection below reads global curves, and rewriting a located
// edge's pcurve table would change the meaning of the other located pcurves.
inline TopoDS_Wire fc_wire_on_surface(const TopoDS_Wire& wire,
                                      const Handle(Geom_Surface)& surface)
{
    const Standard_Boolean located = !wire.Location().IsIdentity();
    TopoDS_Wire target = wire;
    if (located) {
        BRepBuilderAPI_Copy copier(wire, Standard_True, Standard_False);
        if (!copier.IsDone() || copier.Shape().ShapeType() != TopAbs_WIRE)
            return TopoDS_Wire();
        target = TopoDS::Wire(copier.Shape());
        // The pcurves below are computed from the globally located 3D curves, so
        // the copy is read in the global frame too.
        target.Location(TopLoc_Location());
    }
    BRep_Builder builder;
    for (TopExp_Explorer it(target, TopAbs_EDGE); it.More(); it.Next()) {
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
    return target;
}

// --- faces from wires ------------------------------------------------------

// `Part.Face(wire)` for a single boundary wire: upstream's
// `BRepBuilderAPI_MakeFace(wire)` with `OnlyPlane = false`, which asks
// `BRepLib_FindSurface` for a plane, cylinder or sphere that supports the whole
// wire.
//
// When that search does not settle on a surface, the wire's own edges are asked
// for the surface they already carry, and the wire is re-projected onto it (see
// below). This is the "existing surface" half of upstream's search, made
// explicit.
inline std::unique_ptr<TopoDS_Shape> fc_face_from_wire(const TopoDS_Shape& wire)
{
    if (wire.IsNull() || wire.ShapeType() != TopAbs_WIRE)
        return nullptr;
    try {
        const TopoDS_Wire w = TopoDS::Wire(wire);
        BRepBuilderAPI_MakeFace make(w);
        if (make.IsDone() && !make.Face().IsNull())
            return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(make.Face()));

        // The surface the wire's own edges already carry: an edge built from a
        // pcurve (`Part.Geom2d.*.toShape(surface)`) is attached to exactly the
        // surface the caller meant. Each such call builds its own surface
        // *object*, though, so the edges of one wire share a description rather
        // than one handle; re-projecting the wire (on a copy, never the caller's
        // edges) puts every pcurve on the first edge's surface.
        TopExp_Explorer first_edge(w, TopAbs_EDGE);
        if (!first_edge.More())
            return nullptr;
        Handle(Geom2d_Curve) pcurve;
        Handle(Geom_Surface) support;
        TopLoc_Location location;
        Standard_Real first = 0.0;
        Standard_Real last = 0.0;
        BRep_Tool::CurveOnSurface(TopoDS::Edge(first_edge.Current()), pcurve, support, location,
                                 first, last);
        if (pcurve.IsNull() || support.IsNull())
            return nullptr;
        BRepBuilderAPI_Copy copier(w, Standard_True, Standard_False);
        if (!copier.IsDone() || copier.Shape().ShapeType() != TopAbs_WIRE)
            return nullptr;
        const TopoDS_Wire projected = fc_wire_on_surface(TopoDS::Wire(copier.Shape()), support);
        if (projected.IsNull())
            return nullptr;
        BRepBuilderAPI_MakeFace on_wire(projected);
        if (on_wire.IsDone() && !on_wire.Face().IsNull())
            return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(on_wire.Face()));
        BRepBuilderAPI_MakeFace on_surface(support, Precision::Confusion());
        if (!on_surface.IsDone())
            return nullptr;
        on_surface.Add(projected);
        if (!on_surface.IsDone() || on_surface.Face().IsNull())
            return nullptr;
        BRepCheck_Analyzer check(on_surface.Face());
        if (!check.IsValid())
            return nullptr;
        return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(on_surface.Face()));
    }
    catch (...) {
        return nullptr;
    }
}

// `BRepBuilderAPI_MakeFace::Add`: attach a further boundary wire to an existing
// face, upstream's `Part.Face([outer, hole…])` loop. The caller has already
// oriented the wire.
inline std::unique_ptr<TopoDS_Shape> fc_face_add_wire(const TopoDS_Shape& face,
                                                      const TopoDS_Shape& wire)
{
    if (face.IsNull() || face.ShapeType() != TopAbs_FACE || wire.IsNull()
        || wire.ShapeType() != TopAbs_WIRE)
        return nullptr;
    try {
        TopoDS_Face result = TopoDS::Face(face);
        BRep_Builder builder;
        builder.Add(result, TopoDS::Wire(wire));
        return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(result));
    }
    catch (...) {
        return nullptr;
    }
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
        // The wire is projected first: unlike upstream's wires, a wire this
        // kernel builds (an extruded patch's outer wire, a polygon) may carry no
        // pcurve on the elementary surface, and `MakeFace` with `Inside=true`
        // needs them to work out which side of the wire bounds the face.
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

// --- FaceMakerBullseye -----------------------------------------------------

// `FaceMakerBullseye::FaceDriller::getWireDirection`: +1 when `wire` already
// runs counter-clockwise about `plane` (it can bound a face), -1 when it runs
// clockwise (it can bound a hole), 0 on failure. The test is upstream's own:
// build the face and see whether OCCT's `CheckInside` flipped the wire.
inline int fc_bullseye_wire_direction(const gp_Pln& plane, const TopoDS_Wire& wire)
{
    try {
        BRepBuilderAPI_MakeFace make(plane, wire, Standard_True);
        if (!make.IsDone())
            return 0;
        const TopoDS_Face face = make.Face();
        TopoDS_Iterator it(face, Standard_False);
        if (face.IsNull() || !it.More())
            return 0;
        return it.Value().Orientation() == wire.Orientation() ? 1 : -1;
    }
    catch (...) {
        return 0;
    }
}

// The `(u, v)` of `wire`'s first vertex on `plane`, with the vertex tolerance —
// the sample point `FaceMakerBullseye::FaceDriller::hitTest` classifies.
inline bool fc_bullseye_sample(const TopoDS_Shape& wire,
                               const gp_Pln& plane,
                               gp_Pnt2d& uv,
                               double& tolerance)
{
    TopExp_Explorer it(wire, TopAbs_VERTEX);
    if (!it.More())
        return false;
    try {
        const TopoDS_Vertex vertex = TopoDS::Vertex(it.Current());
        tolerance = BRep_Tool::Tolerance(vertex);
        GeomAPI_ProjectPointOnSurf projection(BRep_Tool::Pnt(vertex), new Geom_Plane(plane));
        if (projection.NbPoints() < 1)
            return false;
        Standard_Real u = 0.0;
        Standard_Real v = 0.0;
        projection.LowerDistanceParameters(u, v);
        uv.SetCoord(u, v);
        return true;
    }
    catch (...) {
        return false;
    }
}

inline TopAbs_State fc_classify_in_face(const TopoDS_Face& face,
                                        const gp_Pnt2d& uv,
                                        double tolerance)
{
    if (face.IsNull())
        return TopAbs_UNKNOWN;
    try {
        BRepClass_FaceClassifier classifier(face, uv, tolerance);
        return classifier.State();
    }
    catch (...) {
        return TopAbs_UNKNOWN;
    }
}

// `Part.makeFace(wires, "Part::FaceMakerBullseye")`: planar faces with holes,
// where the holes may themselves hold further faces (upstream's
// `FaceMakerBullseye::Build_Essence`).
//
// The wires are sorted by bounding-box extent, largest first, and each one is
// classified against the faces built so far: a wire inside a face is a hole of
// that face, anything else starts a new face. That is exactly upstream's
// single-pass walk (`reuseInnerWire` is false for Bullseye). One face is
// returned directly, several as a compound.
inline std::unique_ptr<TopoDS_Shape> fc_make_face_bullseye(const TopTools_ListOfShape& wires)
{
    if (wires.IsEmpty())
        return nullptr;
    try {
        for (TopTools_ListIteratorOfListOfShape it(wires); it.More(); it.Next()) {
            if (it.Value().ShapeType() != TopAbs_WIRE
                || !BRep_Tool::IsClosed(TopoDS::Wire(it.Value())))
                return nullptr;
        }

        // The shared plane, found from a compound of copies (upstream does the
        // same, so a supplied plane is never assumed).
        TopoDS_Compound compound;
        BRep_Builder builder;
        builder.MakeCompound(compound);
        for (TopTools_ListIteratorOfListOfShape it(wires); it.More(); it.Next())
            builder.Add(compound, BRepBuilderAPI_Copy(it.Value()).Shape());
        BRepLib_FindSurface finder(compound, -1.0, Standard_True);
        if (!finder.Found() || finder.Surface().IsNull())
            return nullptr;
        const Handle(Geom_Plane) plane_surface = Handle(Geom_Plane)::DownCast(finder.Surface());
        if (plane_surface.IsNull())
            return nullptr;
        const gp_Pln plane = plane_surface->Pln();

        struct Item
        {
            TopoDS_Wire wire;
            double extent {};
        };
        std::vector<Item> items;
        for (TopTools_ListIteratorOfListOfShape it(wires); it.More(); it.Next()) {
            Bnd_Box box;
            BRepBndLib::AddOptimal(it.Value(), box, Standard_False);
            if (box.IsVoid())
                continue;
            items.push_back({TopoDS::Wire(it.Value()), box.SquareExtent()});
        }
        std::stable_sort(items.begin(), items.end(), [](const Item& a, const Item& b) {
            return a.extent - b.extent > Precision::Confusion();
        });

        struct Driller
        {
            TopoDS_Face face;
            TopoDS_Face bound;
        };
        std::vector<Driller> faces;
        const Handle(Geom_Plane) surface = new Geom_Plane(plane);
        for (const Item& item : items) {
            gp_Pnt2d uv;
            double tolerance = 0.0;
            if (!fc_bullseye_sample(item.wire, plane, uv, tolerance))
                return nullptr;
            int found = -1;
            for (int index = static_cast<int>(faces.size()) - 1; index >= 0; --index) {
                const TopAbs_State state = fc_classify_in_face(faces[index].face, uv, tolerance);
                if (state == TopAbs_IN) {
                    found = index;
                    break;
                }
                if (state == TopAbs_OUT
                    && fc_classify_in_face(faces[index].bound, uv, tolerance) != TopAbs_IN)
                    continue;
                // ON the face, or inside a hole: upstream keeps looking, and the
                // wire becomes a new face when no outer face claims it.
            }
            if (found < 0) {
                TopoDS_Wire outer = item.wire;
                const int direction = fc_bullseye_wire_direction(plane, outer);
                if (direction == 0)
                    return nullptr;
                if (direction < 0)
                    outer.Reverse();
                Driller driller;
                builder.MakeFace(driller.face, surface, Precision::Confusion());
                builder.Add(driller.face, outer);
                BRepBuilderAPI_MakeFace bound(surface, outer, Standard_True);
                if (!bound.IsDone())
                    return nullptr;
                driller.bound = bound.Face();
                faces.push_back(driller);
                continue;
            }
            TopoDS_Wire hole = item.wire;
            const int direction = fc_bullseye_wire_direction(plane, hole);
            if (direction == 0)
                return nullptr;
            if (direction > 0)
                hole.Reverse();
            builder.Add(faces[found].face, hole);
        }
        if (faces.empty())
            return nullptr;
        if (faces.size() == 1)
            return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(faces.front().face));
        TopoDS_Compound result_compound;
        builder.MakeCompound(result_compound);
        for (const Driller& driller : faces)
            builder.Add(result_compound, driller.face);
        return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(result_compound));
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

// --- face domain, validation and projection --------------------------------

// `Face.isPartOfDomain(u, v)`: whether the parameter pair lies inside the face's
// boundary (`BRepTopAdaptor_FClass2d`), as upstream's `TopoShapeFacePy`.
inline bool fc_face_is_part_of_domain(const TopoDS_Shape& face, double u, double v)
{
    if (face.IsNull() || face.ShapeType() != TopAbs_FACE)
        return false;
    try {
        BRepTopAdaptor_FClass2d classifier(TopoDS::Face(face), Precision::Confusion());
        const TopAbs_State state = classifier.Perform(gp_Pnt2d(u, v));
        return state == TopAbs_ON || state == TopAbs_IN;
    }
    catch (...) {
        return false;
    }
}

// `Face.validate()`: repair an invalid face's wires in place, exactly as
// upstream's `TopoShapeFacePy::validate` (rebuild each wire with `ShapeFix_Wire`,
// then fall back to `ShapeFix_Shape` when the rebuilt face is still invalid).
// Returns the (possibly unchanged) face.
inline std::unique_ptr<TopoDS_Shape> fc_validate_face(const TopoDS_Shape& shape)
{
    if (shape.IsNull() || shape.ShapeType() != TopAbs_FACE)
        return nullptr;
    try {
        const TopoDS_Face face = TopoDS::Face(shape);
        BRepCheck_Analyzer checker(face);
        if (checker.IsValid())
            return std::make_unique<TopoDS_Shape>(shape);

        const TopoDS_Wire outer = BRepTools::OuterWire(face);
        TopTools_IndexedMapOfShape seen;
        seen.Add(outer);

        TopExp_Explorer xp(face, TopAbs_WIRE);
        ShapeFix_Wire fix;
        fix.SetFace(face);
        fix.Load(outer);
        fix.Perform();
        BRepBuilderAPI_MakeFace mkFace(fix.WireAPIMake());
        while (xp.More()) {
            if (!seen.Contains(xp.Current())) {
                fix.Load(TopoDS::Wire(xp.Current()));
                fix.Perform();
                mkFace.Add(fix.WireAPIMake());
            }
            xp.Next();
        }

        checker.Init(mkFace.Face());
        if (!checker.IsValid()) {
            ShapeFix_Shape fixer(mkFace.Face());
            fixer.SetPrecision(Precision::Confusion());
            fixer.SetMaxTolerance(Precision::Confusion());
            fixer.Perform();
            fixer.FixWireTool()->Perform();
            fixer.FixFaceTool()->Perform();
            return std::make_unique<TopoDS_Shape>(fixer.Shape());
        }
        return std::make_unique<TopoDS_Shape>(mkFace.Face());
    }
    catch (...) {
        return nullptr;
    }
}

// `Shape.fix(precision, mintol, maxtol)`: `ShapeFix_Shape` with upstream's
// per-kind tool walk (`TopoShape::fix(double, double, double)`), returning the
// repaired shape.
inline std::unique_ptr<TopoDS_Shape> fc_fix_shape(const TopoDS_Shape& shape,
                                                   double precision,
                                                   double mintol,
                                                   double maxtol)
{
    if (shape.IsNull())
        return nullptr;
    try {
        const TopAbs_ShapeEnum type = shape.ShapeType();
        ShapeFix_Shape fix(shape);
        fix.SetPrecision(precision);
        fix.SetMinTolerance(mintol);
        fix.SetMaxTolerance(maxtol);
        fix.Perform();

        if (type == TopAbs_SOLID) {
            if (auto tool = fix.FixWireTool())
                tool->Perform();
            if (auto tool = fix.FixFaceTool())
                tool->Perform();
            if (auto tool = fix.FixShellTool())
                tool->Perform();
            if (auto tool = fix.FixSolidTool()) {
                tool->Perform();
                if (!tool->Shape().IsNull())
                    return std::make_unique<TopoDS_Shape>(tool->Shape());
            }
            return std::make_unique<TopoDS_Shape>(fix.Shape());
        }
        if (type == TopAbs_SHELL) {
            if (auto tool = fix.FixWireTool())
                tool->Perform();
            if (auto tool = fix.FixFaceTool())
                tool->Perform();
            if (auto tool = fix.FixShellTool()) {
                tool->Perform();
                if (!tool->Shape().IsNull())
                    return std::make_unique<TopoDS_Shape>(tool->Shape());
            }
            return std::make_unique<TopoDS_Shape>(fix.Shape());
        }
        if (type == TopAbs_FACE) {
            if (auto tool = fix.FixWireTool())
                tool->Perform();
            if (auto tool = fix.FixFaceTool())
                tool->Perform();
            return std::make_unique<TopoDS_Shape>(fix.Shape());
        }
        if (type == TopAbs_WIRE) {
            if (auto tool = fix.FixWireTool())
                tool->Perform();
        }
        return std::make_unique<TopoDS_Shape>(fix.Shape());
    }
    catch (Standard_Failure&) {
        return nullptr;
    }
    catch (...) {
        return nullptr;
    }
}

// The `(u, v)` of `point` on a face's surface (`ShapeAnalysis_Surface::ValueOfUV`,
// what upstream's `GeometrySurfacePy::parameter` calls). False when the surface
// cannot be read or the projection fails.
inline bool fc_face_parameter(const TopoDS_Shape& shape,
                              double x,
                              double y,
                              double z,
                              double& u,
                              double& v)
{
    if (shape.IsNull() || shape.ShapeType() != TopAbs_FACE)
        return false;
    try {
        TopLoc_Location location;
        const Handle(Geom_Surface) surface = BRep_Tool::Surface(TopoDS::Face(shape), location);
        if (surface.IsNull())
            return false;
        ShapeAnalysis_Surface analysis(surface);
        const gp_Pnt2d uv = analysis.ValueOfUV(gp_Pnt(x, y, z), Precision::Confusion());
        u = uv.X();
        v = uv.Y();
        return true;
    }
    catch (...) {
        return false;
    }
}

// `Shape.makeParallelProjection(tool, direction)` / `makePerspectiveProjection`:
// `BRepProj_Projection` of `tool` onto `shape`. Parallel uses a direction,
// perspective a viewpoint.
inline std::unique_ptr<TopoDS_Shape> fc_project_shape(const TopoDS_Shape& tool,
                                                      const TopoDS_Shape& target,
                                                      double x,
                                                      double y,
                                                      double z,
                                                      bool parallel)
{
    if (tool.IsNull() || target.IsNull())
        return nullptr;
    try {
        if (parallel) {
            if (x * x + y * y + z * z <= Precision::Confusion())
                return nullptr;
            BRepProj_Projection made(tool, target, gp_Dir(x, y, z));
            return std::make_unique<TopoDS_Shape>(made.Shape());
        }
        BRepProj_Projection made(tool, target, gp_Pnt(x, y, z));
        return std::make_unique<TopoDS_Shape>(made.Shape());
    }
    catch (...) {
        return nullptr;
    }
}

// --- defeaturing -----------------------------------------------------------

// `Shape.defeaturing(faces)`: remove the given faces and let their neighbours
// grow to close the gap (`BRepAlgoAPI_Defeaturing`), as upstream's
// `TopoShape::makeElementDefeaturing`.
inline std::unique_ptr<TopoDS_Shape> fc_defeature(const TopoDS_Shape& shape,
                                                  const TopTools_ListOfShape& faces)
{
    if (shape.IsNull() || faces.IsEmpty())
        return nullptr;
    try {
        BRepAlgoAPI_Defeaturing defeaturing;
        defeaturing.SetRunParallel(Standard_True);
        defeaturing.SetToFillHistory(Standard_True);
        defeaturing.SetShape(shape);
        for (TopTools_ListIteratorOfListOfShape it(faces); it.More(); it.Next()) {
            if (it.Value().ShapeType() != TopAbs_FACE)
                return nullptr;
            defeaturing.AddFaceToRemove(it.Value());
        }
        defeaturing.Build();
        if (!defeaturing.IsDone() || defeaturing.Shape().IsNull())
            return nullptr;
        return std::make_unique<TopoDS_Shape>(defeaturing.Shape());
    }
    catch (...) {
        return nullptr;
    }
}

// --- 3D offsetting ---------------------------------------------------------

static inline GeomAbs_JoinType fc_offset_join_type(int join)
{
    switch (join) {
        case 1:
            return GeomAbs_Tangent;
        case 2:
            return GeomAbs_Intersection;
        default:
            return GeomAbs_Arc;
    }
}

// `Shape.makeOffsetShape(offset, tolerance, inter, selfInter, offsetMode, join,
// fill)` — a 3D offset (`BRepOffsetAPI_MakeOffsetShape::PerformByJoin`).
//
// With `fill` the free boundary of the input is closed by a wall between the
// original wire and its offset image, the pieces are sewn and, when that gives a
// closed shell, turned into a solid. This mirrors upstream's
// `TopoShape::makeElementOffset`; `offsetMode` is `BRepOffset_Mode` (0 skin,
// 1 pipe, 2 recto-verso) and `join` 0 arcs, 1 tangent, 2 intersection.
inline std::unique_ptr<TopoDS_Shape> fc_offset_shape(const TopoDS_Shape& shape,
                                                     double offset,
                                                     double tolerance,
                                                     bool inter,
                                                     bool self_inter,
                                                     int32_t offset_mode,
                                                     int32_t join,
                                                     bool fill)
{
    if (shape.IsNull())
        return nullptr;
    try {
        BRepOffsetAPI_MakeOffsetShape mkOffset;
        mkOffset.PerformByJoin(
            shape,
            offset,
            tolerance,
            static_cast<BRepOffset_Mode>(offset_mode),
            inter ? Standard_True : Standard_False,
            self_inter ? Standard_True : Standard_False,
            fc_offset_join_type(join)
        );
        if (!mkOffset.IsDone())
            return nullptr;
        TopoDS_Shape result = mkOffset.Shape();
        if (result.IsNull())
            return nullptr;

        // An offset of a solid that loses its solidity is closed back into one.
        auto has_type = [](const TopoDS_Shape& s, TopAbs_ShapeEnum type) {
            for (TopExp_Explorer xp(s, type); xp.More(); xp.Next())
                return true;
            return false;
        };
        if (has_type(shape, TopAbs_SOLID) && !has_type(result, TopAbs_SOLID)) {
            BRepBuilderAPI_Sewing sewn;
            sewn.Add(result);
            sewn.Perform();
            const TopoDS_Shape shell = sewn.SewedShape();
            if (shell.ShapeType() == TopAbs_SHELL) {
                BRepBuilderAPI_MakeSolid solid(TopoDS::Shell(shell));
                if (solid.IsDone())
                    result = solid.Solid();
            }
        }

        if (!fill)
            return std::make_unique<TopoDS_Shape>(result);

        ShapeAnalysis_FreeBoundsProperties free_check(shape);
        free_check.Perform();
        if (free_check.NbClosedFreeBounds() < 1)
            return nullptr;

        BRep_Builder builder;
        const BRepAlgo_Image& image = mkOffset.MakeOffset().OffsetEdgesFromShapes();
        TopoDS_Compound perimeter;
        builder.MakeCompound(perimeter);
        for (int index = 1; index <= free_check.NbClosedFreeBounds(); ++index) {
            const TopoDS_Wire original =
                TopoDS::Wire(free_check.ClosedFreeBound(index)->FreeBound());
            TopoDS_Wire offset_wire;
            builder.MakeWire(offset_wire);
            for (TopExp_Explorer it(original, TopAbs_EDGE); it.More(); it.Next()) {
                if (!image.HasImage(it.Current()))
                    return nullptr;
                const TopTools_ListOfShape& mapped = image.Image(it.Current());
                TopoDS_Edge edge;
                int edges = 0;
                for (TopTools_ListIteratorOfListOfShape list(mapped); list.More(); list.Next()) {
                    if (list.Value().ShapeType() != TopAbs_EDGE)
                        continue;
                    ++edges;
                    edge = TopoDS::Edge(list.Value());
                }
                if (edges != 1)
                    return nullptr;
                builder.Add(offset_wire, edge);
            }
            BRepOffsetAPI_ThruSections generator;
            generator.AddWire(original);
            generator.AddWire(offset_wire);
            generator.Build();
            if (!generator.IsDone())
                return nullptr;
            builder.Add(perimeter, generator.Shape());
        }

        BRepBuilderAPI_Sewing sew;
        sew.Add(shape);
        sew.Add(perimeter);
        sew.Add(result);
        sew.Perform();
        TopoDS_Shape output = sew.SewedShape();
        if (output.ShapeType() == TopAbs_SHELL && output.Closed()) {
            BRepBuilderAPI_MakeSolid solid(TopoDS::Shell(output));
            if (solid.IsDone()) {
                TopoDS_Solid candidate = solid.Solid();
                if (BRepLib::OrientClosedSolid(candidate))
                    output = candidate;
            }
        }
        return std::make_unique<TopoDS_Shape>(output);
    }
    catch (...) {
        return nullptr;
    }
}

// --- wire traversal --------------------------------------------------------

// The wire's edges in connection order (`BRepTools_WireExplorer`), as a compound
// whose direct children are the edges (`Wire.OrderedEdges`).
inline std::unique_ptr<TopoDS_Shape> fc_wire_ordered_edges(const TopoDS_Shape& shape)
{
    if (shape.IsNull() || shape.ShapeType() != TopAbs_WIRE)
        return nullptr;
    try {
        TopoDS_Compound compound;
        BRep_Builder builder;
        builder.MakeCompound(compound);
        for (BRepTools_WireExplorer xp(TopoDS::Wire(shape)); xp.More(); xp.Next())
            builder.Add(compound, xp.Current());
        return std::make_unique<TopoDS_Shape>(compound);
    }
    catch (...) {
        return nullptr;
    }
}

// --- B-spline surfaces -----------------------------------------------------

// The flat layout shared by the four B-spline surface entry points:
//
//   0 u degree, 1 v degree, 2 u periodic, 3 v periodic,
//   4 u first, 5 u last, 6 v first, 7 v last,
//   8 number of u poles, 9 number of v poles,
//   then poles in u-major order: for each u, for each v, `x y z weight`,
//   then the number of u knots followed by (knot, multiplicity) pairs,
//   then the number of v knots followed by (knot, multiplicity) pairs.
//
// The bounds are output-only (`fc_bspline_surface_data`) and ignored on input.
static inline Standard_Boolean fc_read_bspline_surface(rust::Slice<const double> data,
                                                       Handle(Geom_BSplineSurface)& surface,
                                                       Standard_Real& u1,
                                                       Standard_Real& u2,
                                                       Standard_Real& v1,
                                                       Standard_Real& v2)
{
    if (data.size() < 10)
        return Standard_False;
    const int udegree = static_cast<int>(data[0]);
    const int vdegree = static_cast<int>(data[1]);
    const Standard_Boolean uperiodic = data[2] != 0.0;
    const Standard_Boolean vperiodic = data[3] != 0.0;
    const int nbu = static_cast<int>(data[8]);
    const int nbv = static_cast<int>(data[9]);
    if (udegree < 1 || vdegree < 1 || nbu < 2 || nbv < 2)
        return Standard_False;

    std::size_t at = 10;
    const std::size_t poles_end = at + static_cast<std::size_t>(4 * nbu * nbv);
    if (data.size() < poles_end + 2)
        return Standard_False;

    TColgp_Array2OfPnt poles(1, nbu, 1, nbv);
    TColStd_Array2OfReal weights(1, nbu, 1, nbv);
    for (int i = 1; i <= nbu; ++i) {
        for (int j = 1; j <= nbv; ++j) {
            poles.SetValue(i, j, gp_Pnt(data[at], data[at + 1], data[at + 2]));
            weights.SetValue(i, j, data[at + 3]);
            at += 4;
        }
    }
    if (data.size() < at + 1)
        return Standard_False;
    const int nbuk = static_cast<int>(data[at++]);
    if (nbuk < 2 || data.size() < at + static_cast<std::size_t>(2 * nbuk) + 1)
        return Standard_False;
    TColStd_Array1OfReal uknots(1, nbuk);
    TColStd_Array1OfInteger umults(1, nbuk);
    for (int i = 1; i <= nbuk; ++i) {
        uknots.SetValue(i, data[at]);
        umults.SetValue(i, static_cast<int>(data[at + 1]));
        at += 2;
    }
    const int nbvk = static_cast<int>(data[at++]);
    if (nbvk < 2 || data.size() < at + static_cast<std::size_t>(2 * nbvk))
        return Standard_False;
    TColStd_Array1OfReal vknots(1, nbvk);
    TColStd_Array1OfInteger vmults(1, nbvk);
    for (int i = 1; i <= nbvk; ++i) {
        vknots.SetValue(i, data[at]);
        vmults.SetValue(i, static_cast<int>(data[at + 1]));
        at += 2;
    }

    surface = new Geom_BSplineSurface(
        poles,
        weights,
        uknots,
        vknots,
        umults,
        vmults,
        udegree,
        vdegree,
        uperiodic,
        vperiodic
    );
    u1 = uknots.Value(1);
    u2 = uknots.Value(nbuk);
    v1 = vknots.Value(1);
    v2 = vknots.Value(nbvk);
    return Standard_True;
}

// Read a B-spline face's surface into the flat layout above (`Face.Surface`).
// Empty for any other surface kind.
inline rust::Vec<double> fc_bspline_surface_data(const TopoDS_Shape& shape)
{
    rust::Vec<double> out;
    if (shape.IsNull() || shape.ShapeType() != TopAbs_FACE)
        return out;
    try {
        TopLoc_Location location;
        const Handle(Geom_Surface) generic = BRep_Tool::Surface(TopoDS::Face(shape), location);
        const Handle(Geom_BSplineSurface) surface = Handle(Geom_BSplineSurface)::DownCast(generic);
        if (surface.IsNull())
            return out;

        const int nbu = surface->NbUPoles();
        const int nbv = surface->NbVPoles();
        Standard_Real u1 = 0.0, u2 = 0.0, v1 = 0.0, v2 = 0.0;
        surface->Bounds(u1, u2, v1, v2);

        out.push_back(surface->UDegree());
        out.push_back(surface->VDegree());
        out.push_back(surface->IsUPeriodic() ? 1.0 : 0.0);
        out.push_back(surface->IsVPeriodic() ? 1.0 : 0.0);
        out.push_back(u1);
        out.push_back(u2);
        out.push_back(v1);
        out.push_back(v2);
        out.push_back(nbu);
        out.push_back(nbv);

        for (int i = 1; i <= nbu; ++i) {
            for (int j = 1; j <= nbv; ++j) {
                const gp_Pnt point = surface->Pole(i, j);
                out.push_back(point.X());
                out.push_back(point.Y());
                out.push_back(point.Z());
                out.push_back(surface->Weight(i, j));
            }
        }

        out.push_back(surface->NbUKnots());
        for (int i = 1; i <= surface->NbUKnots(); ++i) {
            out.push_back(surface->UKnot(i));
            out.push_back(surface->UMultiplicity(i));
        }
        out.push_back(surface->NbVKnots());
        for (int i = 1; i <= surface->NbVKnots(); ++i) {
            out.push_back(surface->VKnot(i));
            out.push_back(surface->VMultiplicity(i));
        }
        return out;
    }
    catch (...) {
        return rust::Vec<double>();
    }
}

// `BSplineSurface.buildFromPolesMultsKnots`: the surface's face over its whole
// parameter domain (`BRepBuilderAPI_MakeFace`), or null.
inline std::unique_ptr<TopoDS_Shape> fc_bspline_surface_face_from_data(
    rust::Slice<const double> data)
{
    Handle(Geom_BSplineSurface) surface;
    Standard_Real u1 = 0.0, u2 = 0.0, v1 = 0.0, v2 = 0.0;
    if (!fc_read_bspline_surface(data, surface, u1, u2, v1, v2) || surface.IsNull())
        return nullptr;
    try {
        BRepBuilderAPI_MakeFace make(surface, u1, u2, v1, v2, Precision::Confusion());
        if (!make.IsDone())
            return nullptr;
        return std::make_unique<TopoDS_Shape>(make.Face());
    }
    catch (...) {
        return nullptr;
    }
}

// `BSplineSurface.value(u, v)`.
inline bool fc_bspline_surface_value(rust::Slice<const double> data,
                                     double u,
                                     double v,
                                     double& x,
                                     double& y,
                                     double& z)
{
    Handle(Geom_BSplineSurface) surface;
    Standard_Real u1 = 0.0, u2 = 0.0, v1 = 0.0, v2 = 0.0;
    if (!fc_read_bspline_surface(data, surface, u1, u2, v1, v2) || surface.IsNull())
        return false;
    try {
        const gp_Pnt point = surface->Value(u, v);
        x = point.X();
        y = point.Y();
        z = point.Z();
        return true;
    }
    catch (...) {
        return false;
    }
}

// `BSplineSurface.parameter(point)` (`ShapeAnalysis_Surface::ValueOfUV`).
inline bool fc_bspline_surface_parameter(rust::Slice<const double> data,
                                         double x,
                                         double y,
                                         double z,
                                         double& u,
                                         double& v)
{
    Handle(Geom_BSplineSurface) surface;
    Standard_Real u1 = 0.0, u2 = 0.0, v1 = 0.0, v2 = 0.0;
    if (!fc_read_bspline_surface(data, surface, u1, u2, v1, v2) || surface.IsNull())
        return false;
    try {
        ShapeAnalysis_Surface analysis(surface);
        const gp_Pnt2d uv = analysis.ValueOfUV(gp_Pnt(x, y, z), Precision::Confusion());
        u = uv.X();
        v = uv.Y();
        return true;
    }
    catch (...) {
        return false;
    }
}

// `Part.Face(bspline_surface, wires)`: trim the surface with the given boundary
// wires.
//
// Upstream's `TopoShapeFacePyImp` passes the wire straight to
// `BRepBuilderAPI_MakeFace(Geom_Surface, wire)`; OCCT then fills in whatever
// pcurves it needs from the 3-D curves. Projecting the wire first
// (`fc_wire_on_surface`) is *not* upstream and rejects a wire that only lies on
// a free-form surface to within the fitting tolerance.
inline std::unique_ptr<TopoDS_Shape> fc_face_on_bspline_surface(
    rust::Slice<const double> data,
    const TopTools_ListOfShape& wires)
{
    Handle(Geom_BSplineSurface) surface;
    Standard_Real u1 = 0.0, u2 = 0.0, v1 = 0.0, v2 = 0.0;
    if (!fc_read_bspline_surface(data, surface, u1, u2, v1, v2) || surface.IsNull())
        return nullptr;
    if (wires.IsEmpty())
        return nullptr;
    try {
        TopTools_ListIteratorOfListOfShape it(wires);
        if (!it.More() || it.Value().ShapeType() != TopAbs_WIRE)
            return nullptr;
        const Handle(Geom_Surface) support = surface;
        BRepBuilderAPI_MakeFace make(support, TopoDS::Wire(it.Value()), Standard_True);
        if (!make.IsDone())
            return nullptr;
        for (it.Next(); it.More(); it.Next()) {
            if (it.Value().ShapeType() != TopAbs_WIRE)
                return nullptr;
            make.Add(TopoDS::Wire(it.Value()));
            if (!make.IsDone())
                return nullptr;
        }
        if (make.Face().IsNull())
            return nullptr;
        return std::make_unique<TopoDS_Shape>(make.Face());
    }
    catch (...) {
        return nullptr;
    }
}

// The `TopAbs_ShapeEnum` discriminant of a shape (`TopoDS_Shape::ShapeType`),
// or -1 for a null shape. This is the exact topological kind, unlike the
// sub-shape-count inference `Shape.ShapeType` uses.
inline int32_t fc_shape_kind(const TopoDS_Shape& shape)
{
    if (shape.IsNull())
        return -1;
    return static_cast<int32_t>(shape.ShapeType());
}
