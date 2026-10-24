#pragma once

#include <BOPAlgo_Operation.hxx>
#include <BRepAlgoAPI_Common.hxx>
#include <BRepAlgoAPI_Cut.hxx>
#include <BRepAlgoAPI_Fuse.hxx>
#include <BRepBndLib.hxx>
#include <BRepBuilderAPI_MakeShape.hxx>
#include <BRepTools.hxx>
#include <BRep_Builder.hxx>
#include <Bnd_Box.hxx>
#include <Message_ProgressRange.hxx>
#include <Precision.hxx>
#include <TopTools_IndexedMapOfShape.hxx>
#include <TopTools_ListIteratorOfListOfShape.hxx>
#include <TopTools_ListOfShape.hxx>
#include <TopoDS_Compound.hxx>
#include <TopoDS_Iterator.hxx>
#include <TopoDS_Shape.hxx>

#include <cmath>
#include <memory>
#include <sstream>
#include <string>

#include "rust/cxx.h"

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

// --- one reusable cut operation ---------------------------------------------
//
// Upstream's `FCBRepAlgoAPI_BooleanOperation::Build` reuses a *single* operation
// object and calls `Build()` several times: once to fuse a compound tool, then
// for the cut, or once per child when the base is a compound. Its history
// therefore describes the last internal build. These helpers expose the setters
// the crate does not bridge so the same policy can be driven from Rust.

inline std::unique_ptr<BRepAlgoAPI_Cut> fc_cut_new_op()
{
    return std::make_unique<BRepAlgoAPI_Cut>();
}

inline void fc_cut_set_operation(BRepAlgoAPI_Cut& op, bool fuse)
{
    op.SetOperation(fuse ? BOPAlgo_FUSE : BOPAlgo_CUT);
}

inline void fc_cut_configure(BRepAlgoAPI_Cut& op, const TopTools_ListOfShape& arguments,
                             const TopTools_ListOfShape& tools)
{
    op.SetArguments(arguments);
    op.SetTools(tools);
}

inline void fc_cut_build(BRepAlgoAPI_Cut& op)
{
    Message_ProgressRange range;
    op.Build(range);
}

// Upstream's `FCBRepAlgoAPI_BooleanOperation` constructor settings:
// non-destructive so the operands are not rewritten in place, and an automatic
// fuzzy value derived from the combined bounds.
inline void fc_cut_prepare(BRepAlgoAPI_Cut& op)
{
    op.SetRunParallel(Standard_True);
    op.SetNonDestructive(Standard_True);
}

inline void fc_cut_set_auto_fuzzy(BRepAlgoAPI_Cut& op)
{
    Bnd_Box bounds;
    for (TopTools_ListIteratorOfListOfShape it(op.Arguments()); it.More(); it.Next())
        BRepBndLib::Add(it.Value(), bounds);
    for (TopTools_ListIteratorOfListOfShape it(op.Tools()); it.More(); it.Next())
        BRepBndLib::Add(it.Value(), bounds);
    // `FuzzyHelper::getBooleanFuzzy()` defaults to 1.0.
    op.SetFuzzyValue(1.0 * std::sqrt(bounds.SquareExtent()) * Precision::Confusion());
}

// --- shape structure --------------------------------------------------------
// `TopoDS_Iterator` walks the *direct* children of a shape, which is what
// upstream's `RecursiveAddTools`/`RecursiveCutCompound` iterate (unlike
// `TopExp::MapShapes`, which recurses).

inline int fc_shape_child_count(const TopoDS_Shape& shape)
{
    int count = 0;
    for (TopoDS_Iterator it(shape); it.More(); it.Next())
        ++count;
    return count;
}

inline std::unique_ptr<TopoDS_Shape> fc_shape_child(const TopoDS_Shape& shape, int index)
{
    int current = 0;
    for (TopoDS_Iterator it(shape); it.More(); it.Next()) {
        if (++current == index)
            return std::make_unique<TopoDS_Shape>(it.Value());
    }
    return nullptr;
}

// --- BREP persistence -------------------------------------------------------
// In-memory shape <-> bytes. `opencascade-sys` only bridges the file-based
// `BRepTools::write/read`; a property value needs bytes, not a temp file.

inline void fc_brep_write(const TopoDS_Shape& shape, rust::Vec<uint8_t>& out)
{
    std::ostringstream stream;
    BRepTools::Write(shape, stream);
    const std::string bytes = stream.str();
    for (char c : bytes)
        out.push_back(static_cast<uint8_t>(c));
}

inline std::unique_ptr<TopoDS_Shape> fc_brep_read(rust::Slice<const uint8_t> data)
{
    const std::string bytes(reinterpret_cast<const char*>(data.data()), data.size());
    std::istringstream stream(bytes);
    auto shape = std::make_unique<TopoDS_Shape>();
    BRep_Builder builder;
    BRepTools::Read(*shape, stream, builder);
    if (shape->IsNull())
        return nullptr;
    return shape;
}
