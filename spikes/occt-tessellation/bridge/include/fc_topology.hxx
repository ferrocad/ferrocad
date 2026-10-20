#pragma once

// FerroCAD spike: the topology calls `opencascade-sys` 0.3 does not bridge.
//
// Kept in the spike until the findings settle the real `fc_topology.hxx` in
// `crates/ferrocad_occt/include/`.

#include <BRepBuilderAPI_MakeShape.hxx>
#include <BRepFilletAPI_MakeChamfer.hxx>
#include <BRepFilletAPI_MakeFillet.hxx>
#include <TopTools_IndexedMapOfShape.hxx>
#include <TopTools_ListIteratorOfListOfShape.hxx>
#include <TopTools_ListOfShape.hxx>
#include <TopoDS_Edge.hxx>
#include <TopoDS_Face.hxx>
#include <TopoDS_Shape.hxx>

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

// --- asymmetric chamfer ------------------------------------------------------
// The crate bridges only `Add(distance, edge)`. The two-distance form needs the
// adjacent face, which is why the caller passes it explicitly.

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
inline void fc_fill_modified(Op& op, const TopoDS_Shape& s,
                             TopTools_IndexedMapOfShape& out)
{
    out.Clear();
    const TopTools_ListOfShape& list = op.Modified(s);
    for (TopTools_ListIteratorOfListOfShape it(list); it.More(); it.Next())
        out.Add(it.Value());
}

template <typename Op>
inline void fc_fill_generated(Op& op, const TopoDS_Shape& s,
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
    fc_fill_modified(op, s, out);
}
inline void fc_fillet_generated(BRepFilletAPI_MakeFillet& op, const TopoDS_Shape& s,
                                TopTools_IndexedMapOfShape& out)
{
    fc_fill_generated(op, s, out);
}
inline bool fc_fillet_is_deleted(BRepFilletAPI_MakeFillet& op, const TopoDS_Shape& s)
{
    return op.IsDeleted(s);
}

inline void fc_chamfer_modified(BRepFilletAPI_MakeChamfer& op, const TopoDS_Shape& s,
                                TopTools_IndexedMapOfShape& out)
{
    fc_fill_modified(op, s, out);
}
inline void fc_chamfer_generated(BRepFilletAPI_MakeChamfer& op, const TopoDS_Shape& s,
                                 TopTools_IndexedMapOfShape& out)
{
    fc_fill_generated(op, s, out);
}
inline bool fc_chamfer_is_deleted(BRepFilletAPI_MakeChamfer& op, const TopoDS_Shape& s)
{
    return op.IsDeleted(s);
}
