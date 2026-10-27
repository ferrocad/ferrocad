#pragma once

// FerroCAD: the topology calls `opencascade-sys` 0.3 does not bridge.
//
// Companion to `fc_history.hxx`. Verified in `spikes/occt-tessellation/`; see
// `docs/developer/interaction/topology-selection.md` §5.
//
// All functions are `inline`, so they are emitted into the cxx-generated shim and no
// separate translation unit is needed.

#include <BRepAdaptor_Curve.hxx>
#include <BRepAdaptor_Surface.hxx>
#include <BRepBuilderAPI_Copy.hxx>
#include <BRepBuilderAPI_MakeShape.hxx>
#include <BRepExtrema_DistShapeShape.hxx>
#include <BRepFilletAPI_MakeChamfer.hxx>
#include <BRepFilletAPI_MakeFillet.hxx>
#include <BRepTools.hxx>
#include <BRep_Tool.hxx>
#include <GeomAbs_Shape.hxx>
#include <GeomAbs_SurfaceType.hxx>
#include <TopAbs_ShapeEnum.hxx>
#include <TopExp.hxx>
#include <TopExp_Explorer.hxx>
#include <TopTools_IndexedDataMapOfShapeListOfShape.hxx>
#include <TopTools_IndexedMapOfShape.hxx>
#include <TopTools_ListIteratorOfListOfShape.hxx>
#include <TopTools_ListOfShape.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Edge.hxx>
#include <TopoDS_Face.hxx>
#include <TopoDS_Iterator.hxx>
#include <TopoDS_Shape.hxx>
#include <TopoDS_Vertex.hxx>
#include <gp_Pnt.hxx>
#include <gp_Vec.hxx>

#include <algorithm>
#include <stdexcept>
#include <functional>
#include <memory>

#include "rust/cxx.h"

// --- shape identity ----------------------------------------------------------

// A deep copy with a new TShape (`BRepBuilderAPI_Copy`), which is what upstream
// `Shape.copy()` is. The geometry is copied too, so the result is no longer
// `IsSame`/`IsEqual` with the source but has the same volume and area.
inline std::unique_ptr<TopoDS_Shape> fc_copy_shape(const TopoDS_Shape& shape)
{
    if (shape.IsNull())
        return nullptr;
    BRepBuilderAPI_Copy copier(shape, Standard_True, Standard_False);
    if (!copier.IsDone())
        return nullptr;
    return std::make_unique<TopoDS_Shape>(copier.Shape());
}

// A hash consistent with `TopoDS_Shape::IsSame`: the TShape pointer and the
// location, ignoring the orientation. Two handles to one sub-shape hash alike even
// when one was reversed, which is what SimpleCAD's `hash(edge)` buckets need.
inline uint64_t fc_shape_hash(const TopoDS_Shape& shape, uint64_t upper)
{
    if (shape.IsNull() || upper == 0)
        return 0;
    size_t hash = std::hash<const void*>{}(shape.TShape().get());
    const size_t location = shape.Location().HashCode();
    hash ^= location + 0x9e3779b97f4a7c15ULL + (hash << 6) + (hash >> 2);
    return static_cast<uint64_t>(hash % upper);
}

// --- ancestors ---------------------------------------------------------------
// `TopExp::MapShapesAndAncestors` is bridged, but the resulting
// `TopTools_ListOfShape` exposes only `Append`/`Size` in Rust.

inline int fc_list_size(const TopTools_ListOfShape& list) { return list.Size(); }

inline const TopoDS_Shape& fc_list_value(const TopTools_ListOfShape& list, int index)
{
    if (index < 1)
        throw std::out_of_range("fc_list_value: index is 1-based");
    TopTools_ListIteratorOfListOfShape it(list);
    for (int i = 1; it.More(); it.Next(), ++i)
        if (i == index)
            return it.Value();
    throw std::out_of_range("fc_list_value: index out of range");
}

// `TopTools_IndexedMapOfShape::FindIndex` compares orientation and location. An
// ancestor list stores shapes with the orientation they have in the parent, so the
// same edge can differ in orientation from the map key. `IsSame` ignores orientation.
inline int fc_indexed_map_find_index_same(const TopTools_IndexedMapOfShape& map,
                                          const TopoDS_Shape& shape)
{
    for (int i = 1; i <= map.Extent(); ++i)
        if (map.FindKey(i).IsSame(shape))
            return i;
    return 0;
}

inline int fc_data_map_find_index_same(
    const TopTools_IndexedDataMapOfShapeListOfShape& map,
    const TopoDS_Shape& shape)
{
    for (int i = 1; i <= map.Extent(); ++i)
        if (map.FindKey(i).IsSame(shape))
            return i;
    return 0;
}

// --- shape queries -----------------------------------------------------------
// `BRep_Tool::Degenerated`, `BRep_Tool::IsClosed(edge, face)`, `BRep_Tool::Range`
// and `BRepTools::UVBounds` are not bridged by the crate.

inline bool fc_edge_degenerated(const TopoDS_Shape& shape)
{
    return BRep_Tool::Degenerated(TopoDS::Edge(shape));
}

inline bool fc_edge_is_seam(const TopoDS_Shape& edge, const TopoDS_Shape& face)
{
    return BRep_Tool::IsClosed(TopoDS::Edge(edge), TopoDS::Face(face));
}

inline void fc_edge_range(const TopoDS_Shape& shape, double& first, double& last)
{
    BRep_Tool::Range(TopoDS::Edge(shape), first, last);
}

inline void fc_face_uv_bounds(const TopoDS_Shape& shape,
                              double& u0,
                              double& u1,
                              double& v0,
                              double& v1)
{
    BRepTools::UVBounds(TopoDS::Face(shape), u0, u1, v0, v1);
}

inline int fc_direct_child_count(const TopoDS_Shape& shape)
{
    int count = 0;
    for (TopoDS_Iterator it(shape); it.More(); it.Next())
        ++count;
    return count;
}

// --- geometry queries --------------------------------------------------------
// `BRepExtrema_DistShapeShape`, `BRepAdaptor_Surface::Value`,
// `BRepAdaptor_Curve::Value` and `BRep_Tool::Tolerance` are not bridged by the
// crate.

// The minimum distance between two shapes and the closest point on each.
// `first`/`second` receive the point on the first/second shape respectively.
// Returns false when the extrema solver reports no solution.
inline bool fc_dist_to_shape(const TopoDS_Shape& first,
                             const TopoDS_Shape& second,
                             double& distance,
                             double& first_x,
                             double& first_y,
                             double& first_z,
                             double& second_x,
                             double& second_y,
                             double& second_z)
{
    if (first.IsNull() || second.IsNull())
        return false;
    BRepExtrema_DistShapeShape dist(first, second);
    if (!dist.IsDone() || dist.NbSolution() < 1)
        return false;
    distance = dist.Value();
    const gp_Pnt p1 = dist.PointOnShape1(1);
    const gp_Pnt p2 = dist.PointOnShape2(1);
    first_x = p1.X();
    first_y = p1.Y();
    first_z = p1.Z();
    second_x = p2.X();
    second_y = p2.Y();
    second_z = p2.Z();
    return true;
}

// Upstream `TopoShape::isClosed` (`Mod/Part/App/TopoShape.cpp`): a shell, wire
// or edge asks `BRep_Tool::IsClosed`; a solid or comp-solid requires every shell
// to be closed; a compound also requires every face, wire, edge and vertex that
// is not contained in a higher element to be closed; a face or vertex falls
// through to `BRep_Tool::IsClosed`, i.e. its own `Closed` flag.
inline bool fc_is_closed(const TopoDS_Shape& shape)
{
    if (shape.IsNull())
        return false;
    bool closed = false;
    switch (shape.ShapeType())
    {
        case TopAbs_SHELL:
        case TopAbs_WIRE:
        case TopAbs_EDGE:
        case TopAbs_FACE:
        case TopAbs_VERTEX:
        case TopAbs_SHAPE:
            closed = BRep_Tool::IsClosed(shape);
            break;
        case TopAbs_COMPSOLID:
        case TopAbs_SOLID:
            closed = true;
            for (TopExp_Explorer xp(shape, TopAbs_SHELL); xp.More(); xp.Next())
                closed = closed && BRep_Tool::IsClosed(xp.Current());
            break;
        case TopAbs_COMPOUND:
            closed = true;
            for (TopExp_Explorer xp(shape, TopAbs_SHELL); xp.More(); xp.Next())
                closed = closed && BRep_Tool::IsClosed(xp.Current());
            for (TopExp_Explorer xp(shape, TopAbs_FACE, TopAbs_SHELL); xp.More(); xp.Next())
                closed = closed && BRep_Tool::IsClosed(xp.Current());
            for (TopExp_Explorer xp(shape, TopAbs_WIRE, TopAbs_FACE); xp.More(); xp.Next())
                closed = closed && BRep_Tool::IsClosed(xp.Current());
            for (TopExp_Explorer xp(shape, TopAbs_EDGE, TopAbs_WIRE); xp.More(); xp.Next())
                closed = closed && BRep_Tool::IsClosed(xp.Current());
            for (TopExp_Explorer xp(shape, TopAbs_VERTEX, TopAbs_EDGE); xp.More(); xp.Next())
                closed = closed && BRep_Tool::IsClosed(xp.Current());
            break;
    }
    return closed;
}

// The geometric tolerance of a shape (`BRep_Tool::Tolerance`). The crate only
// exposes it per vertex, edge and face, so a higher shape reports the maximum
// over its sub-shapes, matching `ShapeAnalysis_ShapeTolerance`'s default.
inline double fc_shape_tolerance(const TopoDS_Shape& shape)
{
    if (shape.IsNull())
        return 0.0;
    switch (shape.ShapeType())
    {
        case TopAbs_VERTEX:
            return BRep_Tool::Tolerance(TopoDS::Vertex(shape));
        case TopAbs_EDGE:
            return BRep_Tool::Tolerance(TopoDS::Edge(shape));
        case TopAbs_FACE:
            return BRep_Tool::Tolerance(TopoDS::Face(shape));
        default:
            break;
    }
    double tolerance = 0.0;
    for (TopExp_Explorer xp(shape, TopAbs_VERTEX); xp.More(); xp.Next())
        tolerance = std::max(tolerance, BRep_Tool::Tolerance(TopoDS::Vertex(xp.Current())));
    for (TopExp_Explorer xp(shape, TopAbs_EDGE); xp.More(); xp.Next())
        tolerance = std::max(tolerance, BRep_Tool::Tolerance(TopoDS::Edge(xp.Current())));
    for (TopExp_Explorer xp(shape, TopAbs_FACE); xp.More(); xp.Next())
        tolerance = std::max(tolerance, BRep_Tool::Tolerance(TopoDS::Face(xp.Current())));
    return tolerance;
}

// The 3D point of an edge at curve parameter `parameter`
// (`BRepAdaptor_Curve::Value`; upstream `Edge.valueAt`).
inline bool fc_edge_value(const TopoDS_Shape& shape,
                          double parameter,
                          double& x,
                          double& y,
                          double& z)
{
    try
    {
        BRepAdaptor_Curve curve(TopoDS::Edge(shape));
        const gp_Pnt p = curve.Value(parameter);
        x = p.X();
        y = p.Y();
        z = p.Z();
        return true;
    }
    catch (...)
    {
        return false;
    }
}

// The 3D point of a face at surface parameters `(u, v)`
// (`BRepAdaptor_Surface::Value`; upstream `Face.valueAt`).
inline bool fc_face_value(const TopoDS_Shape& shape,
                          double u,
                          double v,
                          double& x,
                          double& y,
                          double& z)
{
    try
    {
        BRepAdaptor_Surface surface(TopoDS::Face(shape));
        const gp_Pnt p = surface.Value(u, v);
        x = p.X();
        y = p.Y();
        z = p.Z();
        return true;
    }
    catch (...)
    {
        return false;
    }
}

// The face's backing surface, in the frame `BRepAdaptor_Surface` evaluates with
// (so parameters derived from it round-trip through `fc_face_value`). The
// first value tags the kind, followed by the frame:
//
//   1  plane     origin xyz, x-direction xyz, y-direction xyz, normal xyz  (13)
//   2  cylinder  origin xyz, axis xyz, x-direction xyz, radius             (11)
//   3  sphere    centre xyz, axis xyz, x-direction xyz, radius             (11)
//
// Any other surface (cone, torus, B-spline, …) leaves `out` empty; the caller
// keeps the type id instead.
inline void fc_face_surface_data(const TopoDS_Shape& shape, rust::Vec<double>& out)
{
    try
    {
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

        switch (surface.GetType())
        {
            case GeomAbs_Plane:
            {
                const gp_Ax3 pos = surface.Plane().Position();
                out.push_back(1.0);
                push_point(pos.Location());
                push_dir(pos.XDirection());
                push_dir(pos.YDirection());
                push_dir(pos.Direction());
                break;
            }
            case GeomAbs_Cylinder:
            {
                const gp_Cylinder cylinder = surface.Cylinder();
                const gp_Ax3 pos = cylinder.Position();
                out.push_back(2.0);
                push_point(pos.Location());
                push_dir(pos.Direction());
                push_dir(pos.XDirection());
                out.push_back(cylinder.Radius());
                break;
            }
            case GeomAbs_Sphere:
            {
                const gp_Sphere sphere = surface.Sphere();
                const gp_Ax3 pos = sphere.Position();
                out.push_back(3.0);
                push_point(pos.Location());
                push_dir(pos.Direction());
                push_dir(pos.XDirection());
                out.push_back(sphere.Radius());
                break;
            }
            default:
                break;
        }
    }
    catch (...)
    {
        out.clear();
    }
}

// --- asymmetric chamfer ------------------------------------------------------
// The crate bridges only `Add(distance, edge)`. The two-distance form needs the
// adjacent face; note OCCT's argument order is (d1, d2, edge, face).

inline void fc_chamfer_add_asym(BRepFilletAPI_MakeChamfer& op,
                                double distance1,
                                double distance2,
                                const TopoDS_Edge& edge,
                                const TopoDS_Face& face)
{
    op.Add(distance1, distance2, edge, face);
}

// --- fillet / chamfer lineage -------------------------------------------------
// `BRepFilletAPI_MakeFillet` and `_MakeChamfer` derive from
// `BRepBuilderAPI_MakeShape`, whose history methods the crate does not bridge.

template <typename Op>
inline void fc_topology_fill_modified(Op& op, const TopoDS_Shape& s,
                                      TopTools_IndexedMapOfShape& out)
{
    out.Clear();
    const TopTools_ListOfShape& list = op.Modified(s);
    for (TopTools_ListIteratorOfListOfShape it(list); it.More(); it.Next())
        out.Add(it.Value());
}

template <typename Op>
inline void fc_topology_fill_generated(Op& op, const TopoDS_Shape& s,
                                       TopTools_IndexedMapOfShape& out)
{
    out.Clear();
    const TopTools_ListOfShape& list = op.Generated(s);
    for (TopTools_ListIteratorOfListOfShape it(list); it.More(); it.Next())
        out.Add(it.Value());
}

inline void fc_fillet_modified(BRepFilletAPI_MakeFillet& op, const TopoDS_Shape& s,
                               TopTools_IndexedMapOfShape& out)
{
    fc_topology_fill_modified(op, s, out);
}
inline void fc_fillet_generated(BRepFilletAPI_MakeFillet& op, const TopoDS_Shape& s,
                                TopTools_IndexedMapOfShape& out)
{
    fc_topology_fill_generated(op, s, out);
}
inline bool fc_fillet_is_deleted(BRepFilletAPI_MakeFillet& op, const TopoDS_Shape& s)
{
    return op.IsDeleted(s);
}

inline void fc_chamfer_modified(BRepFilletAPI_MakeChamfer& op, const TopoDS_Shape& s,
                                TopTools_IndexedMapOfShape& out)
{
    fc_topology_fill_modified(op, s, out);
}
inline void fc_chamfer_generated(BRepFilletAPI_MakeChamfer& op, const TopoDS_Shape& s,
                                 TopTools_IndexedMapOfShape& out)
{
    fc_topology_fill_generated(op, s, out);
}
inline bool fc_chamfer_is_deleted(BRepFilletAPI_MakeChamfer& op, const TopoDS_Shape& s)
{
    return op.IsDeleted(s);
}

// --- edge tangents ------------------------------------------------------------
// Tangent continuity between two adjacent edges at their shared vertex. The crate
// bridges neither `BRepAdaptor_Curve::D1` nor `BRep_Tool::Parameter`, and
// `BRep_Tool::Continuity` is surface continuity across one edge, not this.
//
// Returns six numbers (two unit tangents); an empty vector means the edges do not
// share a vertex or a tangent is degenerate.

inline void fc_edge_tangents(const TopoDS_Edge& edge1,
                             const TopoDS_Edge& edge2,
                             rust::Vec<double>& out)
{
    TopoDS_Vertex vertex;
    if (!TopExp::CommonVertex(edge1, edge2, vertex))
        return;

    const TopoDS_Edge edges[2] = {edge1, edge2};
    for (int i = 0; i < 2; ++i)
    {
        BRepAdaptor_Curve curve(edges[i]);
        const double u = BRep_Tool::Parameter(vertex, edges[i]);
        gp_Pnt point;
        gp_Vec derivative;
        curve.D1(u, point, derivative);
        const double magnitude = derivative.Magnitude();
        if (magnitude < 1e-12)
            return;
        out.push_back(derivative.X() / magnitude);
        out.push_back(derivative.Y() / magnitude);
        out.push_back(derivative.Z() / magnitude);
    }
}
