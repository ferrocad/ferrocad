#pragma once

// FerroCAD: the topology calls `opencascade-sys` 0.3 does not bridge.
//
// Companion to `fc_history.hxx`. Verified in `spikes/occt-tessellation/`; see
// `docs/topology-selection.md` §5.
//
// All functions are `inline`, so they are emitted into the cxx-generated shim and no
// separate translation unit is needed.

#include <BRepAdaptor_Curve.hxx>
#include <BRepBuilderAPI_MakeShape.hxx>
#include <BRepFilletAPI_MakeChamfer.hxx>
#include <BRepFilletAPI_MakeFillet.hxx>
#include <BRep_Tool.hxx>
#include <GeomAbs_Shape.hxx>
#include <TopExp.hxx>
#include <TopTools_IndexedDataMapOfShapeListOfShape.hxx>
#include <TopTools_IndexedMapOfShape.hxx>
#include <TopTools_ListIteratorOfListOfShape.hxx>
#include <TopTools_ListOfShape.hxx>
#include <TopoDS_Edge.hxx>
#include <TopoDS_Face.hxx>
#include <TopoDS_Shape.hxx>
#include <TopoDS_Vertex.hxx>
#include <gp_Pnt.hxx>
#include <gp_Vec.hxx>

#include <stdexcept>

#include "rust/cxx.h"

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
