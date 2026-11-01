#pragma once

// FerroCAD: the OCCT calls whose *result* is a FreeCAD `TopoShape` policy
// decision, not a plain kernel answer. They stay in this crate on purpose - see
// `crates/ferrocad_part_cxx/README.md` in ferrocad/ferrocad_occt_cxx: a function
// that encodes what FreeCAD would do belongs here, not in the bindings crate.
//
// They are still C++ because each one needs an OCCT entry point the bindings do
// not expose yet:
//
//   fc_cut_prepare        BRepAlgoAPI_BooleanOperation::SetRunParallel/
//                         SetNonDestructive
//   fc_cut_set_auto_fuzzy BRepAlgoAPI_BooleanOperation::Arguments/Tools/
//                         SetFuzzyValue, Bnd_Box::SquareExtent
//   fc_is_closed          BRep_Tool::IsClosed(shape)
//   fc_shape_tolerance    BRep_Tool::Tolerance on arbitrary shapes
//
// Once those land in `ferrocad_part_cxx`, the bodies below become Rust.

#include <BRepAlgoAPI_Cut.hxx>
#include <BRepBndLib.hxx>
#include <BRep_Tool.hxx>
#include <Bnd_Box.hxx>
#include <Precision.hxx>
#include <TopAbs_ShapeEnum.hxx>
#include <TopExp_Explorer.hxx>
#include <TopTools_ListIteratorOfListOfShape.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Edge.hxx>
#include <TopoDS_Face.hxx>
#include <TopoDS_Shape.hxx>
#include <TopoDS_Vertex.hxx>

#include <algorithm>
#include <cmath>

// --- boolean-operation policy ------------------------------------------------
// FreeCAD's `FCBRepAlgoAPI_BooleanOperation` constructor settings: non-destructive
// so the operands are not rewritten in place, and an automatic fuzzy value derived
// from the combined bounds.

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

// --- shape predicates --------------------------------------------------------
// FreeCAD `TopoShape::isClosed` (Mod/Part/App/TopoShape.cpp): a shell, wire or
// edge asks `BRep_Tool::IsClosed`; a solid or comp-solid requires every shell to
// be closed; a compound also requires every face, wire, edge and vertex that is
// not contained in a higher element to be closed; a face or vertex falls through
// to `BRep_Tool::IsClosed`, i.e. its own `Closed` flag.

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
