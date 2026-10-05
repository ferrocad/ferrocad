#pragma once

#include <BRepAlgoAPI_Common.hxx>
#include <BRepAlgoAPI_Cut.hxx>
#include <BRepAlgoAPI_Fuse.hxx>
#include <BRepBuilderAPI_MakeShape.hxx>
#include <TopTools_IndexedMapOfShape.hxx>
#include <TopTools_ListIteratorOfListOfShape.hxx>
#include <TopTools_ListOfShape.hxx>
#include <TopoDS_Shape.hxx>

// FerroCAD: the history / element-map calls that `opencascade-sys` 0.3 does not
// bridge, provided as a *sibling* cxx bridge over the crate's own types.
// No fork, no patch, no registry edit. Verified in spikes/occt-history-rs.
//
// All functions are `inline`, so they are emitted into the cxx-generated shim and
// no separate translation unit is needed. They live in the global namespace to
// match the (namespace-less) bridge in `opencascade-sys`, so the shared opaque
// types keep their names.

// `TopTools_IndexedMapOfShape::FindIndex` — shape -> 1-based index in this map.
inline int fc_indexed_map_find_index(const TopTools_IndexedMapOfShape& map,
                                     const TopoDS_Shape& shape)
{
    return map.FindIndex(shape);
}

// Modified output sub-shapes of `s`, de-duplicated into `out` (Empty if none).
template <typename Op>
inline void fc_fill_modified(Op& op, const TopoDS_Shape& s,
                             TopTools_IndexedMapOfShape& out)
{
    out.Clear();
    const TopTools_ListOfShape& lst = op.Modified(s);
    for (TopTools_ListIteratorOfListOfShape it(lst); it.More(); it.Next())
        out.Add(it.Value());
}

// Generated output sub-shapes of `s`, de-duplicated into `out`.
template <typename Op>
inline void fc_fill_generated(Op& op, const TopoDS_Shape& s,
                              TopTools_IndexedMapOfShape& out)
{
    out.Clear();
    const TopTools_ListOfShape& lst = op.Generated(s);
    for (TopTools_ListIteratorOfListOfShape it(lst); it.More(); it.Next())
        out.Add(it.Value());
}

// --- BRepAlgoAPI_Fuse -------------------------------------------------------

inline void fc_brep_fuse_modified(BRepAlgoAPI_Fuse& op, const TopoDS_Shape& s,
                                  TopTools_IndexedMapOfShape& out)
{
    fc_fill_modified(op, s, out);
}

inline void fc_brep_fuse_generated(BRepAlgoAPI_Fuse& op, const TopoDS_Shape& s,
                                   TopTools_IndexedMapOfShape& out)
{
    fc_fill_generated(op, s, out);
}

inline bool fc_brep_fuse_is_deleted(BRepAlgoAPI_Fuse& op, const TopoDS_Shape& s)
{
    return op.IsDeleted(s);
}

// --- BRepAlgoAPI_Cut --------------------------------------------------------

inline void fc_brep_cut_modified(BRepAlgoAPI_Cut& op, const TopoDS_Shape& s,
                                 TopTools_IndexedMapOfShape& out)
{
    fc_fill_modified(op, s, out);
}

inline void fc_brep_cut_generated(BRepAlgoAPI_Cut& op, const TopoDS_Shape& s,
                                  TopTools_IndexedMapOfShape& out)
{
    fc_fill_generated(op, s, out);
}

inline bool fc_brep_cut_is_deleted(BRepAlgoAPI_Cut& op, const TopoDS_Shape& s)
{
    return op.IsDeleted(s);
}

// --- BRepAlgoAPI_Common -----------------------------------------------------

inline void fc_brep_common_modified(BRepAlgoAPI_Common& op, const TopoDS_Shape& s,
                                    TopTools_IndexedMapOfShape& out)
{
    fc_fill_modified(op, s, out);
}

inline void fc_brep_common_generated(BRepAlgoAPI_Common& op, const TopoDS_Shape& s,
                                     TopTools_IndexedMapOfShape& out)
{
    fc_fill_generated(op, s, out);
}

inline bool fc_brep_common_is_deleted(BRepAlgoAPI_Common& op, const TopoDS_Shape& s)
{
    return op.IsDeleted(s);
}
