// OCCT history smoke probe for FerroCAD's lineage / topological-naming spike.
//
// Goal: confirm, empirically, that OCCT exposes enough *shape history* to build a
// stable element map (the thing topological naming needs). It builds two boxes,
// fuses them, then fillets one edge, and prints, for every input sub-shape, what
// the kernel reports: Modified, Generated, Deleted. It also reads the
// BRepTools_History object the boolean operation keeps.
//
// This is a throwaway probe. It is deliberately NOT part of the Cargo workspace.
// It answers a kernel question (does the history exist and what does it look like),
// independent of which Rust binding we eventually pick.
//
// Build (OCCT >= 7.8 dev headers + libraries must be installed):
//   cmake -S . -B build && cmake --build build -j && ./build/occ_history_probe
//
// Expected reading: after a fuse, coincident faces are typically MODIFIED (one
// face survives, two map into it), new faces where the boxes meet are GENERATED,
// and no input face is outright deleted in a simple overlap. The fillet should
// report the filleted edge as DELETED or MODIFIED and generate a new face. If any
// of those three lists is always empty, the kernel is not giving us a lineage and
// the element-map plan has to change.

#include <iostream>

#include <BRepAlgoAPI_Fuse.hxx>
#include <BRepBuilderAPI_MakeShape.hxx>
#include <BRepFilletAPI_MakeFillet.hxx>
#include <BRepPrimAPI_MakeBox.hxx>
#include <BRepTools_History.hxx>
#include <TopExp.hxx>
#include <TopExp_Explorer.hxx>
#include <TopTools_IndexedMapOfShape.hxx>
#include <TopTools_ListIteratorOfListOfShape.hxx>
#include <TopTools_ListOfShape.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Edge.hxx>
#include <TopoDS_Shape.hxx>
#include <gp_Pnt.hxx>

static const char* kindName(TopAbs_ShapeEnum kind)
{
    switch (kind) {
        case TopAbs_FACE:
            return "Face";
        case TopAbs_EDGE:
            return "Edge";
        case TopAbs_VERTEX:
            return "Vertex";
        case TopAbs_SOLID:
            return "Solid";
        case TopAbs_SHELL:
            return "Shell";
        case TopAbs_COMPOUND:
            return "Compound";
        default:
            return "Shape";
    }
}

static void printCounts(const char* label, const TopoDS_Shape& shape)
{
    std::cout << label << ":";
    for (TopAbs_ShapeEnum kind : {TopAbs_SOLID, TopAbs_FACE, TopAbs_EDGE, TopAbs_VERTEX}) {
        TopTools_IndexedMapOfShape map;
        TopExp::MapShapes(shape, kind, map);
        std::cout << " " << map.Extent() << " " << kindName(kind) << "s";
    }
    std::cout << "\n";
}

// One line per input sub-shape: what did the operation do to it?
static void reportHistory(
    const char* phase,
    BRepBuilderAPI_MakeShape& maker,
    const TopoDS_Shape& input,
    TopAbs_ShapeEnum kind
)
{
    TopTools_IndexedMapOfShape map;
    TopExp::MapShapes(input, kind, map);
    std::cout << "-- " << phase << " history over " << map.Extent() << " " << kindName(kind)
              << "(s) of the input:\n";
    for (int i = 1; i <= map.Extent(); ++i) {
        const TopoDS_Shape& s = map(i);
        const TopTools_ListOfShape& modified = maker.Modified(s);
        const TopTools_ListOfShape& generated = maker.Generated(s);
        const Standard_Boolean deleted = maker.IsDeleted(s);

        std::cout << "   " << kindName(kind) << i << ":";
        if (deleted) {
            std::cout << " DELETED";
        }
        if (!modified.IsEmpty()) {
            std::cout << " Modified->" << modified.Extent();
        }
        if (!generated.IsEmpty()) {
            std::cout << " Generated->" << generated.Extent();
        }
        if (!deleted && modified.IsEmpty() && generated.IsEmpty()) {
            std::cout << " unchanged/unreported";
        }
        std::cout << "\n";
    }
}

int main()
{
    std::cout << "OCCT history probe\n==================\n";

    TopoDS_Shape a = BRepPrimAPI_MakeBox(gp_Pnt(0, 0, 0), 10, 10, 10).Shape();
    TopoDS_Shape b = BRepPrimAPI_MakeBox(gp_Pnt(5, 0, 0), 10, 10, 10).Shape();

    std::cout << "\n== inputs ==\n";
    printCounts("box a", a);
    printCounts("box b", b);

    // --- Fuse -----------------------------------------------------------------
    BRepAlgoAPI_Fuse fuse(a, b);
    if (!fuse.IsDone()) {
        std::cerr << "fuse failed\n";
        return 2;
    }
    TopoDS_Shape fused = fuse.Shape();
    std::cout << "\n== fuse output ==\n";
    printCounts("fused", fused);

    reportHistory("FUSE", fuse, a, TopAbs_FACE);
    reportHistory("FUSE", fuse, a, TopAbs_EDGE);
    reportHistory("FUSE", fuse, b, TopAbs_FACE);
    reportHistory("FUSE", fuse, b, TopAbs_EDGE);

    // The binary `Modified`/`Generated` accessors come from BRepBuilderAPI_MakeShape.
    // The same operation also keeps a richer BRepTools_History (this is what
    // FreeCAD's MapperHistory wraps).
    Handle(BRepTools_History) history = fuse.History();
    if (!history.IsNull()) {
        TopTools_IndexedMapOfShape faces;
        TopExp::MapShapes(a, TopAbs_FACE, faces);
        int removed = 0;
        int modified = 0;
        int generated = 0;
        for (int i = 1; i <= faces.Extent(); ++i) {
            const TopoDS_Shape& s = faces(i);
            if (history->IsRemoved(s)) {
                ++removed;
            }
            if (!history->Modified(s).IsEmpty()) {
                ++modified;
            }
            if (!history->Generated(s).IsEmpty()) {
                ++generated;
            }
        }
        std::cout << "\n-- BRepTools_History over box a faces: " << removed << " removed, "
                  << modified << " modified, " << generated << " generated\n";
    }
    else {
        std::cout << "\n-- BRepTools_History: null (boolean kept no history object)\n";
    }

    // --- Fillet ---------------------------------------------------------------
    TopTools_IndexedMapOfShape fusedEdges;
    TopExp::MapShapes(fused, TopAbs_EDGE, fusedEdges);
    if (fusedEdges.Extent() > 0) {
        BRepFilletAPI_MakeFillet fillet(fused);
        fillet.Add(1.0, TopoDS::Edge(fusedEdges(1)));
        fillet.Build();
        if (fillet.IsDone()) {
            std::cout << "\n== fillet output ==\n";
            printCounts("filleted", fillet.Shape());
            reportHistory("FILLET", fillet, fused, TopAbs_FACE);
            reportHistory("FILLET", fillet, fused, TopAbs_EDGE);
        }
        else {
            std::cout << "\nfillet failed\n";
        }
    }

    // --- The naming question --------------------------------------------------
    // The kernel names nothing. It only reports relationships above. A stable
    // element name (Face6, ";FUSED;...") has to be *assigned* by us from this
    // history and persisted with the shape. This probe confirms the raw material
    // is there; the mapping is our code (see docs/geometry-and-topology.md).
    std::cout << "\n(note: OCCT never names elements; only history is reported)\n";
    return 0;
}
