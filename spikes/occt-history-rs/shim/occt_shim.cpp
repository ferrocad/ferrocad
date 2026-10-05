// C ABI implementation over OCCT. Compiled by build.rs via the `cc` crate.
//
// Every OCCT class/method here is the same one FreeCAD's element mapper consumes
// (verified against src/Mod/Part/App). See docs/occt-history-spike.md.
#include "occt_shim.h"

#include <memory>

#include <BRepAlgoAPI_Fuse.hxx>
#include <BRepBuilderAPI_MakeShape.hxx>
#include <BRepFilletAPI_MakeFillet.hxx>
#include <BRepPrimAPI_MakeBox.hxx>
#include <TopExp.hxx>
#include <TopTools_IndexedMapOfShape.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Shape.hxx>
#include <gp_Pnt.hxx>

struct OcctShape {
    TopoDS_Shape shape;
};

// Owns the operation so its history accessors stay valid after the call returns.
// `BRepAlgoAPI_Fuse` and `BRepFilletAPI_MakeFillet` both derive from
// `BRepBuilderAPI_MakeShape`, which is where Modified/Generated/IsDeleted live.
struct OcctHistory {
    std::unique_ptr<BRepBuilderAPI_MakeShape> op;
};

static TopAbs_ShapeEnum kind_of(int kind)
{
    switch (kind) {
        case OCCT_KIND_SOLID:
            return TopAbs_SOLID;
        case OCCT_KIND_FACE:
            return TopAbs_FACE;
        case OCCT_KIND_EDGE:
            return TopAbs_EDGE;
        default:
            return TopAbs_VERTEX;
    }
}

extern "C" {

OcctShape* occt_make_box(double x, double y, double z, double dx, double dy, double dz)
{
    return new OcctShape {BRepPrimAPI_MakeBox(gp_Pnt(x, y, z), dx, dy, dz).Shape()};
}

OcctShape* occt_fuse(const OcctShape* a, const OcctShape* b, OcctHistory** history_out)
{
    if (history_out) {
        *history_out = nullptr;
    }
    if (!a || !b) {
        return nullptr;
    }
    auto op = std::make_unique<BRepAlgoAPI_Fuse>(a->shape, b->shape);
    if (!op->IsDone()) {
        return nullptr;
    }
    TopoDS_Shape result = op->Shape();
    if (history_out) {
        *history_out = new OcctHistory {std::move(op)};
    }
    return new OcctShape {result};
}

OcctShape* occt_fillet(
    const OcctShape* shape,
    int edge_ordinal,
    double radius,
    OcctHistory** history_out
)
{
    if (history_out) {
        *history_out = nullptr;
    }
    if (!shape) {
        return nullptr;
    }
    TopTools_IndexedMapOfShape edges;
    TopExp::MapShapes(shape->shape, TopAbs_EDGE, edges);
    if (edge_ordinal < 1 || edge_ordinal > edges.Extent()) {
        return nullptr;
    }
    auto op = std::make_unique<BRepFilletAPI_MakeFillet>(shape->shape);
    op->Add(radius, TopoDS::Edge(edges(edge_ordinal)));
    op->Build();
    if (!op->IsDone()) {
        return nullptr;
    }
    TopoDS_Shape result = op->Shape();
    if (history_out) {
        *history_out = new OcctHistory {std::move(op)};
    }
    return new OcctShape {result};
}

void occt_shape_free(OcctShape* shape)
{
    delete shape;
}

void occt_history_free(OcctHistory* history)
{
    delete history;
}

int occt_shape_count(const OcctShape* shape, int kind)
{
    if (!shape) {
        return -1;
    }
    TopTools_IndexedMapOfShape map;
    TopExp::MapShapes(shape->shape, kind_of(kind), map);
    return map.Extent();
}

// Resolve the `ordinal`-th sub-shape (1-based) of `input`, or null.
static const TopoDS_Shape* nth_sub_shape(const OcctShape* input, int kind, int ordinal)
{
    static thread_local TopTools_IndexedMapOfShape map;
    map.Clear();
    TopExp::MapShapes(input->shape, kind_of(kind), map);
    if (ordinal < 1 || ordinal > map.Extent()) {
        return nullptr;
    }
    return &map(ordinal);
}

int occt_history_modified(const OcctHistory* history, const OcctShape* input, int kind, int ordinal)
{
    if (!history || !history->op || !input) {
        return -1;
    }
    const TopoDS_Shape* s = nth_sub_shape(input, kind, ordinal);
    return s ? history->op->Modified(*s).Extent() : -1;
}

int occt_history_generated(const OcctHistory* history, const OcctShape* input, int kind, int ordinal)
{
    if (!history || !history->op || !input) {
        return -1;
    }
    const TopoDS_Shape* s = nth_sub_shape(input, kind, ordinal);
    return s ? history->op->Generated(*s).Extent() : -1;
}

int occt_history_deleted(const OcctHistory* history, const OcctShape* input, int kind, int ordinal)
{
    if (!history || !history->op || !input) {
        return -1;
    }
    const TopoDS_Shape* s = nth_sub_shape(input, kind, ordinal);
    return s ? (history->op->IsDeleted(*s) ? 1 : 0) : -1;
}

}  // extern "C"
