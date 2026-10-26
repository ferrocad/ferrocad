#pragma once

// FerroCAD: the remaining `TopoShape` queries and in-place mutators that
// `opencascade-sys` 0.3 does not bridge.
//
// Companion to `fc_part_ops.hxx`. Everything here mirrors a method of upstream's
// `Part.TopoShape` (see `TopoShapePyImp.cpp` and `TopoShape.cpp` in the pinned
// checkout) so the Python layer never has to invent semantics:
//
//   isInside            BRepExtrema_DistShapeShape / BRepClass3d_SolidClassifier
//   isPartner           TopoDS_Shape::IsPartner
//   complement          TopoDS_Shape::Complement
//   transformGeometry   BRepBuilderAPI_Copy + BRepBuilderAPI_GTransform
//   globalTolerance     ShapeAnalysis_ShapeTolerance
//   fixTolerance        ShapeFix_ShapeTolerance::SetTolerance
//   limitTolerance      ShapeFix_ShapeTolerance::LimitTolerance
//   inTolerance         ShapeAnalysis_ShapeTolerance::InTolerance
//   overTolerance       ShapeAnalysis_ShapeTolerance::OverTolerance
//   makeWires           BRepBuilderAPI_MakeWire + ShapeFix_Wire
//   removeInternalWires ShapeUpgrade_RemoveInternalWires
//   exportStl           BRepMesh_IncrementalMesh + binary STL
//
// Every entry point catches OCCT exceptions and reports failure with `false`, a
// null/`-1` result or an empty vector; no `Standard_Failure` crosses the bridge.

#include <BRepBuilderAPI_Copy.hxx>
#include <BRepBuilderAPI_GTransform.hxx>
#include <BRepBuilderAPI_MakeVertex.hxx>
#include <BRepBuilderAPI_MakeWire.hxx>
#include <BRepClass3d_SolidClassifier.hxx>
#include <BRepExtrema_DistShapeShape.hxx>
#include <BRepMesh_IncrementalMesh.hxx>
#include <BRep_Tool.hxx>
#include <BRep_Builder.hxx>
#include <Poly_Triangulation.hxx>
#include <Precision.hxx>
#include <ShapeAnalysis_ShapeTolerance.hxx>
#include <ShapeFix_ShapeTolerance.hxx>
#include <ShapeFix_Wire.hxx>
#include <ShapeUpgrade_RemoveInternalWires.hxx>
#include <TopAbs_Orientation.hxx>
#include <TopAbs_ShapeEnum.hxx>
#include <TopAbs_State.hxx>
#include <TopExp.hxx>
#include <TopExp_Explorer.hxx>
#include <TopLoc_Location.hxx>
#include <TopTools_HSequenceOfShape.hxx>
#include <TopTools_IndexedMapOfShape.hxx>
#include <TopTools_ListOfShape.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Compound.hxx>
#include <TopoDS_Face.hxx>
#include <TopoDS_Shape.hxx>
#include <TopoDS_Wire.hxx>
#include <gp_GTrsf.hxx>
#include <gp_Pnt.hxx>
#include <gp_Trsf.hxx>
#include <gp_XYZ.hxx>

#include <algorithm>
#include <array>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <limits>
#include <list>
#include <memory>
#include <vector>

#include "rust/cxx.h"

// --- isInside --------------------------------------------------------------

// `Shape.isInside(point, tolerance, checkFace)`.
//
// The two paths are upstream's, not a uniform classifier: a lower-dimensional
// shape (vertex, edge, wire, face) answers with the point-to-shape distance, so
// any point within `tolerance` counts as inside, while a solid, shell or
// compound goes through `BRepClass3d_SolidClassifier` and additionally counts a
// boundary point when `check_face` is set. Returns 1 (inside), 0 (outside) or
// -1 (the kernel failed).
inline int fc_is_inside(const TopoDS_Shape& shape,
                        double x,
                        double y,
                        double z,
                        double tolerance,
                        bool check_face)
{
    if (shape.IsNull() || !std::isfinite(tolerance))
        return -1;
    try {
        const gp_Pnt point(x, y, z);
        const TopAbs_ShapeEnum type = shape.ShapeType();
        if (type == TopAbs_VERTEX || type == TopAbs_EDGE || type == TopAbs_WIRE
            || type == TopAbs_FACE) {
            BRepBuilderAPI_MakeVertex maker(point);
            BRepExtrema_DistShapeShape distance(maker.Vertex(), shape);
            if (!distance.Perform())
                return -1;
            return distance.Value() <= tolerance ? 1 : 0;
        }

        BRepClass3d_SolidClassifier classifier(shape);
        classifier.Perform(point, tolerance);
        bool inside = classifier.State() == TopAbs_IN;
        if (check_face && classifier.IsOnAFace())
            inside = true;
        return inside ? 1 : 0;
    }
    catch (...) {
        return -1;
    }
}

// --- identity --------------------------------------------------------------

// `Shape.isPartner(shape)`: the same TShape, whatever the location/orientation.
inline bool fc_is_partner(const TopoDS_Shape& first, const TopoDS_Shape& second)
{
    return first.IsPartner(second);
}

// `Shape.complement()`: complement the orientation (`Forward`<->`Reversed`,
// `Internal`<->`External`). The source handle is not modified.
inline std::unique_ptr<TopoDS_Shape> fc_complement(const TopoDS_Shape& shape)
{
    if (shape.IsNull())
        return nullptr;
    try {
        TopoDS_Shape copy(shape);
        copy.Complement();
        return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(copy));
    }
    catch (...) {
        return nullptr;
    }
}

// --- transformGeometry -----------------------------------------------------

// `Shape.transformGeometry(matrix, copy)`: apply the affine matrix to the
// underlying curves and surfaces. The source is copied first, as upstream does,
// because `gp_GTrsf::Trsf()` rejects a non-orthogonal transform on the original.
inline std::unique_ptr<TopoDS_Shape> fc_transform_geometry(const TopoDS_Shape& shape,
                                                           rust::Slice<const double> matrix,
                                                           bool copy)
{
    if (shape.IsNull() || matrix.size() < 16)
        return nullptr;
    try {
        gp_GTrsf trsf;
        for (int row = 1; row <= 3; ++row) {
            for (int column = 1; column <= 4; ++column) {
                trsf.SetValue(row, column, matrix[(row - 1) * 4 + (column - 1)]);
            }
        }
        BRepBuilderAPI_Copy copier(shape);
        BRepBuilderAPI_GTransform op(copier.Shape(), trsf, copy ? Standard_True : Standard_False);
        if (!op.IsDone())
            return nullptr;
        const TopoDS_Shape& result = op.Shape();
        if (result.IsNull())
            return nullptr;
        return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(result));
    }
    catch (...) {
        return nullptr;
    }
}

// --- tolerances ------------------------------------------------------------

// `Shape.globalTolerance(mode)`: the average (0), maximum (>0) or minimum (<0)
// tolerance over the vertices, edges and faces, via the kernel's accumulator.
// `NaN` when the kernel cannot compute it (a null shape included).
inline double fc_global_tolerance(const TopoDS_Shape& shape, int mode)
{
    if (shape.IsNull())
        return std::numeric_limits<double>::quiet_NaN();
    try {
        ShapeAnalysis_ShapeTolerance analysis;
        analysis.Tolerance(shape, mode);
        return analysis.GlobalTolerance(mode);
    }
    catch (...) {
        return std::numeric_limits<double>::quiet_NaN();
    }
}

// `Shape.fixTolerance(value, ShapeType)`: force every selected sub-shape's
// tolerance to `value`. The fixer mutates the TShape it is given, which is the
// shared kernel object, exactly as upstream's in-place `SetTolerance` does.
inline bool fc_fix_tolerance(const TopoDS_Shape& shape, double value, int shape_type)
{
    if (shape.IsNull() || !std::isfinite(value) || value < 0.0)
        return false;
    try {
        ShapeFix_ShapeTolerance fix;
        fix.SetTolerance(shape, value, static_cast<TopAbs_ShapeEnum>(shape_type));
        return true;
    }
    catch (...) {
        return false;
    }
}

// `Shape.limitTolerance(tmin, tmax, ShapeType)`: whether any tolerance changed.
inline bool fc_limit_tolerance(const TopoDS_Shape& shape,
                               double minimum,
                               double maximum,
                               int shape_type)
{
    if (shape.IsNull() || !std::isfinite(minimum) || !std::isfinite(maximum))
        return false;
    try {
        ShapeFix_ShapeTolerance fix;
        return fix.LimitTolerance(shape,
                                  minimum,
                                  maximum,
                                  static_cast<TopAbs_ShapeEnum>(shape_type));
    }
    catch (...) {
        return false;
    }
}

// `Shape.inTolerance(valmin, valmax, ShapeType)`: append the matching sub-shapes
// to `out`. `out` is cleared first; a kernel failure leaves it empty rather than
// reporting a partial result.
inline void fc_tolerance_in(const TopoDS_Shape& shape,
                            double minimum,
                            double maximum,
                            int shape_type,
                            TopTools_ListOfShape& out)
{
    out.Clear();
    if (shape.IsNull() || !std::isfinite(minimum) || !std::isfinite(maximum))
        return;
    try {
        ShapeAnalysis_ShapeTolerance analysis;
        const Handle(TopTools_HSequenceOfShape) found =
            analysis.InTolerance(shape,
                                 minimum,
                                 maximum,
                                 static_cast<TopAbs_ShapeEnum>(shape_type));
        if (found.IsNull())
            return;
        for (int index = 1; index <= found->Length(); ++index)
            out.Append(found->Value(index));
    }
    catch (...) {
        out.Clear();
    }
}

// `Shape.overTolerance(value, ShapeType)`: append the matching sub-shapes to
// `out`.
inline void fc_tolerance_over(const TopoDS_Shape& shape,
                              double value,
                              int shape_type,
                              TopTools_ListOfShape& out)
{
    out.Clear();
    if (shape.IsNull() || !std::isfinite(value))
        return;
    try {
        ShapeAnalysis_ShapeTolerance analysis;
        const Handle(TopTools_HSequenceOfShape) found =
            analysis.OverTolerance(shape, value, static_cast<TopAbs_ShapeEnum>(shape_type));
        if (found.IsNull())
            return;
        for (int index = 1; index <= found->Length(); ++index)
            out.Append(found->Value(index));
    }
    catch (...) {
        out.Clear();
    }
}

// --- makeWires -------------------------------------------------------------

// `Shape.makeWires()`: sort the edges into wires and return a compound of all
// of them (upstream's `TopoShape::makeWires`). The edges are read from the
// shape's edge map, so a duplicate edge appears once. Upstream's `op` argument
// only labels the element map, which this seam does not build.
//
// The connect step follows upstream's use of `BRepBuilderAPI_MakeWire::Error()`:
// a `DisconnectedWire` error means the edge was not added, so it stays in the
// pending list for the next wire. `ShapeFix_Wire` then reorders each wire and,
// on the same fixer, repairs connectivity and closure.
inline std::unique_ptr<TopoDS_Shape> fc_make_wires(const TopoDS_Shape& shape,
                                                   double tolerance)
{
    if (shape.IsNull())
        return nullptr;
    try {
        const double precision =
            (std::isfinite(tolerance) && tolerance >= Precision::Confusion())
                ? tolerance
                : Precision::Confusion();

        TopTools_IndexedMapOfShape edges;
        TopExp::MapShapes(shape, TopAbs_EDGE, edges);
        std::list<TopoDS_Edge> pending;
        for (int index = 1; index <= edges.Extent(); ++index)
            pending.push_back(TopoDS::Edge(edges.FindKey(index)));

        std::vector<TopoDS_Wire> wires;
        while (!pending.empty()) {
            BRepBuilderAPI_MakeWire maker;
            maker.Add(pending.front());
            pending.pop_front();
            if (!maker.IsDone())
                continue;

            // A failed `Add` clears `IsDone`, so `Wire()` would throw: keep the
            // last wire that was actually built, as upstream does.
            TopoDS_Wire wire = maker.Wire();
            bool found = false;
            do {
                found = false;
                for (auto it = pending.begin(); it != pending.end(); ++it) {
                    maker.Add(*it);
                    if (maker.Error() != BRepBuilderAPI_DisconnectedWire) {
                        found = true;
                        pending.erase(it);
                        wire = maker.Wire();
                        break;
                    }
                }
            } while (found);

            ShapeFix_Wire fix;
            fix.SetPrecision(precision);
            fix.Load(wire);
            fix.FixReorder();
            // Reorder does not change the edges, so this is the wire handed on;
            // FixConnected/FixClosed may replace it afterwards.
            wires.push_back(fix.Wire());
            fix.FixConnected();
            fix.FixClosed();
            wires.back() = fix.Wire();
        }

        BRep_Builder builder;
        TopoDS_Compound compound;
        builder.MakeCompound(compound);
        for (const TopoDS_Wire& wire : wires)
            builder.Add(compound, wire);
        return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(compound));
    }
    catch (...) {
        return nullptr;
    }
}

// --- removeInternalWires ---------------------------------------------------

// The `ShapeUpgrade_RemoveInternalWires` run behind `Shape.removeInternalWires`.
// It is an object rather than one function because the Python method returns
// `Perform()`'s flag *and* the resulting shape, and `Perform` and `GetResult`
// belong to the same fixer instance.
class FcRemoveInternalWires
{
public:
    FcRemoveInternalWires() = default;

    bool perform(const TopoDS_Shape& shape, double minimum_area)
    {
        myFix.reset();
        if (shape.IsNull() || !std::isfinite(minimum_area))
            return false;
        try {
            myFix.reset(new ShapeUpgrade_RemoveInternalWires(shape));
            myFix->MinArea() = minimum_area;
            return myFix->Perform() ? true : false;
        }
        catch (...) {
            myFix.reset();
            return false;
        }
    }

    std::unique_ptr<TopoDS_Shape> result() const
    {
        if (!myFix)
            return nullptr;
        try {
            const TopoDS_Shape result = myFix->GetResult();
            if (result.IsNull())
                return nullptr;
            return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(result));
        }
        catch (...) {
            return nullptr;
        }
    }

private:
    std::unique_ptr<ShapeUpgrade_RemoveInternalWires> myFix;
};

inline std::unique_ptr<FcRemoveInternalWires> fc_remove_internal_wires_new()
{
    return std::make_unique<FcRemoveInternalWires>();
}

// --- exportStl -------------------------------------------------------------

namespace fc_shape_ops_internal
{
inline void push_u32(rust::Vec<uint8_t>& out, uint32_t value)
{
    out.push_back(static_cast<uint8_t>(value & 0xffu));
    out.push_back(static_cast<uint8_t>((value >> 8) & 0xffu));
    out.push_back(static_cast<uint8_t>((value >> 16) & 0xffu));
    out.push_back(static_cast<uint8_t>((value >> 24) & 0xffu));
}

inline void push_f32(rust::Vec<uint8_t>& out, float value)
{
    uint32_t bits = 0;
    std::memcpy(&bits, &value, sizeof(bits));
    push_u32(out, bits);
}

inline void push_point(rust::Vec<uint8_t>& out, const gp_Pnt& point)
{
    push_f32(out, static_cast<float>(point.X()));
    push_f32(out, static_cast<float>(point.Y()));
    push_f32(out, static_cast<float>(point.Z()));
}
}  // namespace fc_shape_ops_internal

// `Shape.exportStl(filename, deflection)`: the binary STL bytes of `shape`.
//
// Upstream meshes with `BRepMesh_IncrementalMesh(shape, deflection, false,
// defaultAngularDeflection(deflection), true)` and lets `StlAPI_Writer` write the
// file. `StlAPI_Writer` is bound to a filename and is not part of this seam's
// "kernel returns bytes" contract, so the facet loop is here instead and the
// Python layer writes the bytes. The angular deflection formula and the
// orientation handling are upstream's. Empty on failure.
inline rust::Vec<uint8_t> fc_stl_bytes(const TopoDS_Shape& shape, double deflection)
{
    rust::Vec<uint8_t> out;
    if (shape.IsNull() || !std::isfinite(deflection) || deflection <= 0.0)
        return out;
    try {
        // upstream `defaultAngularDeflection`.
        const double angular = std::min(0.1, deflection * 5.0 + 0.005);
        BRepMesh_IncrementalMesh mesh(shape,
                                      deflection,
                                      Standard_False,
                                      angular,
                                      Standard_True);
        if (!mesh.IsDone())
            return out;

        std::vector<std::array<gp_Pnt, 3>> facets;
        for (TopExp_Explorer it(shape, TopAbs_FACE); it.More(); it.Next()) {
            const TopoDS_Face face = TopoDS::Face(it.Current());
            TopLoc_Location location;
            const Handle(Poly_Triangulation) triangulation =
                BRep_Tool::Triangulation(face, location);
            if (triangulation.IsNull())
                continue;
            const gp_Trsf transform = location.Transformation();
            const bool reversed = face.Orientation() == TopAbs_REVERSED;
            for (int index = 1; index <= triangulation->NbTriangles(); ++index) {
                Standard_Integer a = 0;
                Standard_Integer b = 0;
                Standard_Integer c = 0;
                triangulation->Triangle(index).Get(a, b, c);
                gp_Pnt pa = triangulation->Node(a);
                gp_Pnt pb = triangulation->Node(b);
                gp_Pnt pc = triangulation->Node(c);
                pa.Transform(transform);
                pb.Transform(transform);
                pc.Transform(transform);
                // A facet that collapses to a line is not a surface: drop it.
                if (pa.Distance(pb) <= Precision::Confusion()
                    || pb.Distance(pc) <= Precision::Confusion()
                    || pc.Distance(pa) <= Precision::Confusion()) {
                    continue;
                }
                // OCCT keeps the face orientation beside the triangulation; flip
                // the winding so the stored normal points out of the solid.
                facets.push_back(reversed ? std::array<gp_Pnt, 3> {pa, pc, pb}
                                          : std::array<gp_Pnt, 3> {pa, pb, pc});
            }
        }

        for (int index = 0; index < 80; ++index)
            out.push_back(0);
        fc_shape_ops_internal::push_u32(out, static_cast<uint32_t>(facets.size()));
        for (const auto& facet : facets) {
            const gp_XYZ normal =
                (facet[1].XYZ() - facet[0].XYZ()).Crossed(facet[2].XYZ() - facet[0].XYZ());
            const double length = normal.Modulus();
            if (length > 0.0) {
                fc_shape_ops_internal::push_f32(out,
                                                static_cast<float>(normal.X() / length));
                fc_shape_ops_internal::push_f32(out,
                                                static_cast<float>(normal.Y() / length));
                fc_shape_ops_internal::push_f32(out,
                                                static_cast<float>(normal.Z() / length));
            }
            else {
                fc_shape_ops_internal::push_f32(out, 0.0f);
                fc_shape_ops_internal::push_f32(out, 0.0f);
                fc_shape_ops_internal::push_f32(out, 0.0f);
            }
            for (const gp_Pnt& point : facet)
                fc_shape_ops_internal::push_point(out, point);
            out.push_back(0);
            out.push_back(0);
        }
    }
    catch (...) {
        out.clear();
    }
    return out;
}
