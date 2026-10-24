#pragma once

// FerroCAD: the surface and solid operations that close the remaining Part
// seams — partial spheres, variable-radius fillets, filled planar offsets,
// multi-shell solids with cavities, exact-by-construction helices and pipe
// shells.
//
// A sibling cxx bridge like `fc_chfi2d.hxx`: `opencascade-sys` 0.3 does not
// bridge these algorithms, so this header wraps them over the crate's global
// `TopoDS_*` types. Every entry point is `inline`, catches every OCCT
// exception, and returns a null `unique_ptr` on failure; the Rust layer turns
// that into `Part.OCCError` (upstream's `Base.CADKernelError`).

#include <BRepAdaptor_Curve.hxx>
#include <BRepBuilderAPI_MakeEdge.hxx>
#include <BRepBuilderAPI_MakeFace.hxx>
#include <BRepBuilderAPI_MakeSolid.hxx>
#include <BRepBuilderAPI_MakeWire.hxx>
#include <BRepBuilderAPI_Transform.hxx>
#include <BRepClass_FaceClassifier.hxx>
#include <BRepFilletAPI_MakeFillet.hxx>
#include <BRepGProp.hxx>
#include <BRepLib.hxx>
#include <BRepOffsetAPI_MakeOffset.hxx>
#include <BRepOffsetAPI_MakePipeShell.hxx>
#include <BRepPrimAPI_MakeSphere.hxx>
#include <BRepTools_WireExplorer.hxx>
#include <BRep_Builder.hxx>
#include <BRep_Tool.hxx>
#include <GCE2d_MakeSegment.hxx>
#include <GProp_GProps.hxx>
#include <Geom2d_TrimmedCurve.hxx>
#include <Geom_ConicalSurface.hxx>
#include <Geom_CylindricalSurface.hxx>
#include <Geom_Surface.hxx>
#include <Precision.hxx>
#include <TopAbs_State.hxx>
#include <TopExp.hxx>
#include <TopExp_Explorer.hxx>
#include <TopTools_ListIteratorOfListOfShape.hxx>
#include <TopTools_ListOfShape.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Compound.hxx>
#include <TopoDS_Edge.hxx>
#include <TopoDS_Face.hxx>
#include <TopoDS_Iterator.hxx>
#include <TopoDS_Shape.hxx>
#include <TopoDS_Shell.hxx>
#include <TopoDS_Solid.hxx>
#include <TopoDS_Vertex.hxx>
#include <TopoDS_Wire.hxx>
#include <gp_Ax1.hxx>
#include <gp_Ax2.hxx>
#include <gp_Ax2d.hxx>
#include <gp_Ax3.hxx>
#include <gp_Dir.hxx>
#include <gp_Dir2d.hxx>
#include <gp_Pnt.hxx>
#include <gp_Pnt2d.hxx>
#include <gp_Trsf.hxx>

#include <algorithm>
#include <cmath>
#include <limits>
#include <memory>
#include <vector>

#include "rust/cxx.h"

namespace fc_surface_ops_internal
{
constexpr double kPi = 3.14159265358979323846264338327950288;
constexpr double kTwoPi = 2.0 * kPi;

inline double radians(double degrees)
{
    return degrees * kPi / 180.0;
}

inline std::unique_ptr<TopoDS_Shape> owned(const TopoDS_Shape& shape)
{
    if (shape.IsNull()) {
        return nullptr;
    }
    return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(shape));
}

// Upstream's `JoinType` (0 = arc, 1 = tangent, 2 = intersection).
inline GeomAbs_JoinType join_type(int kind)
{
    switch (kind) {
        case 1:
            return GeomAbs_Tangent;
        case 2:
            return GeomAbs_Intersection;
        default:
            return GeomAbs_Arc;
    }
}

inline double face_area(const TopoDS_Face& face)
{
    GProp_GProps props;
    BRepGProp::SurfaceProperties(face, props);
    return std::abs(props.Mass());
}

// The area a closed wire encloses, or 0 for an open wire.
inline double wire_area(const TopoDS_Wire& wire)
{
    BRepBuilderAPI_MakeFace make(wire, Standard_True);
    if (!make.IsDone()) {
        return 0.0;
    }
    return face_area(make.Face());
}

// The edges of `wire` in the order the wire traverses them, each oriented along
// that traversal (`BRepTools_WireExplorer::Current()` returns the stored edge;
// its `Orientation()` is what makes the chain connected).
inline void collect_edges(const TopoDS_Wire& wire, std::vector<TopoDS_Edge>& edges)
{
    for (BRepTools_WireExplorer explorer(wire); explorer.More(); explorer.Next()) {
        const TopoDS_Edge edge = TopoDS::Edge(explorer.Current());
        edges.push_back(explorer.Orientation() == TopAbs_REVERSED ? TopoDS::Edge(edge.Reversed())
                                                                  : edge);
    }
}

// A point on the wire away from its endpoints, for face classification.
inline gp_Pnt wire_point(const std::vector<TopoDS_Edge>& edges)
{
    if (edges.empty()) {
        return gp_Pnt(0.0, 0.0, 0.0);
    }
    BRepAdaptor_Curve curve(edges.front());
    return curve.Value(0.5 * (curve.FirstParameter() + curve.LastParameter()));
}

// The first and last vertex of a traversal-oriented edge chain.
inline bool wire_ends(const std::vector<TopoDS_Edge>& edges, TopoDS_Vertex& first,
                      TopoDS_Vertex& last)
{
    if (edges.empty()) {
        return false;
    }
    TopoDS_Vertex other;
    TopExp::Vertices(edges.front(), first, other);
    TopExp::Vertices(edges.back(), other, last);
    return !first.IsNull() && !last.IsNull();
}

inline bool face_contains(const TopoDS_Face& face, const gp_Pnt& point)
{
    BRepClass_FaceClassifier classifier(face, point, Precision::Confusion());
    return classifier.State() == TopAbs_IN;
}

inline TopoDS_Face build_face(const TopoDS_Wire& outer,
                              const std::vector<TopoDS_Wire>& holes,
                              bool reverse_holes)
{
    BRepBuilderAPI_MakeFace make(outer, Standard_True);
    if (!make.IsDone()) {
        return TopoDS_Face();
    }
    for (const auto& hole : holes) {
        make.Add(reverse_holes ? TopoDS::Wire(hole.Reversed()) : hole);
    }
    return make.IsDone() ? make.Face() : TopoDS_Face();
}

// OCCT wants a hole's boundary opposite to the outer boundary; the wires from
// `BRepOffsetAPI_MakeOffset` are not guaranteed to carry that orientation, so
// try both and keep the one whose area is the outer area minus the holes.
inline TopoDS_Face oriented_face(const TopoDS_Wire& outer,
                                 const std::vector<TopoDS_Wire>& holes)
{
    if (holes.empty()) {
        return build_face(outer, holes, false);
    }
    double expected = wire_area(outer);
    for (const auto& hole : holes) {
        expected -= wire_area(hole);
    }
    expected = std::abs(expected);

    const TopoDS_Face reversed = build_face(outer, holes, true);
    const TopoDS_Face plain = build_face(outer, holes, false);
    if (reversed.IsNull()) {
        return plain;
    }
    if (plain.IsNull()) {
        return reversed;
    }
    const double reverse_error = std::abs(face_area(reversed) - expected);
    const double plain_error = std::abs(face_area(plain) - expected);
    return reverse_error <= plain_error ? reversed : plain;
}

struct WireFace
{
    TopoDS_Wire outer;
    std::vector<TopoDS_Wire> holes;
    TopoDS_Face face;
};

inline std::unique_ptr<TopoDS_Shape> compound_of(const std::vector<TopoDS_Shape>& shapes)
{
    if (shapes.empty()) {
        return nullptr;
    }
    if (shapes.size() == 1) {
        return owned(shapes.front());
    }
    TopoDS_Compound compound;
    BRep_Builder builder;
    builder.MakeCompound(compound);
    for (const auto& shape : shapes) {
        builder.Add(compound, shape);
    }
    return owned(compound);
}

// Build the faces of the planar regions bounded by `wires` (upstream's
// `FaceMakerBullseye` policy): the largest wire starts a face, a wire inside a
// face becomes one of its holes, and a wire inside a hole starts a new face.
// This turns a source/offset pair into the band between them.
inline std::unique_ptr<TopoDS_Shape> filled_regions(const std::vector<TopoDS_Wire>& wires)
{
    struct Entry
    {
        TopoDS_Wire wire;
        double area;
        gp_Pnt point;
    };

    std::vector<Entry> entries;
    for (const auto& wire : wires) {
        const double area = wire_area(wire);
        if (area <= Precision::Confusion()) {
            continue;
        }
        std::vector<TopoDS_Edge> edges;
        collect_edges(wire, edges);
        entries.push_back({wire, area, wire_point(edges)});
    }
    std::sort(entries.begin(), entries.end(), [](const Entry& a, const Entry& b) {
        return a.area > b.area;
    });

    std::vector<WireFace> faces;
    for (const auto& entry : entries) {
        int best = -1;
        double best_area = std::numeric_limits<double>::max();
        for (std::size_t index = 0; index < faces.size(); ++index) {
            const double area = face_area(faces[index].face);
            if (area < best_area && face_contains(faces[index].face, entry.point)) {
                best = static_cast<int>(index);
                best_area = area;
            }
        }
        if (best >= 0) {
            WireFace& host = faces[static_cast<std::size_t>(best)];
            host.holes.push_back(entry.wire);
            host.face = oriented_face(host.outer, host.holes);
            if (host.face.IsNull()) {
                return nullptr;
            }
        }
        else {
            WireFace face;
            face.outer = entry.wire;
            face.face = oriented_face(entry.wire, {});
            if (face.face.IsNull()) {
                return nullptr;
            }
            faces.push_back(face);
        }
    }

    std::vector<TopoDS_Shape> shapes;
    shapes.reserve(faces.size());
    for (const auto& face : faces) {
        shapes.push_back(face.face);
    }
    return compound_of(shapes);
}

// The closed part of `source` plus the offset's closed wires, as bands.
inline std::unique_ptr<TopoDS_Shape> fill_closed(const std::vector<TopoDS_Wire>& source,
                                                 const std::vector<TopoDS_Wire>& offset)
{
    std::vector<TopoDS_Wire> wires;
    for (const auto& wire : source) {
        if (BRep_Tool::IsClosed(wire) && wire_area(wire) > Precision::Confusion()) {
            wires.push_back(wire);
        }
    }
    for (const auto& wire : offset) {
        if (BRep_Tool::IsClosed(wire) && wire_area(wire) > Precision::Confusion()) {
            wires.push_back(wire);
        }
    }
    return filled_regions(wires);
}

// The band between one open source wire and one open offset wire, closed by the
// two straight connectors at the ends.
inline TopoDS_Face open_band(const TopoDS_Wire& source, const TopoDS_Wire& offset)
{
    std::vector<TopoDS_Edge> source_edges;
    std::vector<TopoDS_Edge> offset_edges;
    collect_edges(source, source_edges);
    collect_edges(offset, offset_edges);

    TopoDS_Vertex source_first;
    TopoDS_Vertex source_last;
    TopoDS_Vertex offset_first;
    TopoDS_Vertex offset_last;
    if (!wire_ends(source_edges, source_first, source_last)
        || !wire_ends(offset_edges, offset_first, offset_last)) {
        return TopoDS_Face();
    }

    const gp_Pnt start = BRep_Tool::Pnt(source_first);
    const bool forward =
      start.Distance(BRep_Tool::Pnt(offset_first)) <= start.Distance(BRep_Tool::Pnt(offset_last));
    const TopoDS_Vertex offset_start = forward ? offset_first : offset_last;
    const TopoDS_Vertex offset_end = forward ? offset_last : offset_first;

    BRepBuilderAPI_MakeWire make;
    for (const auto& edge : source_edges) {
        make.Add(edge);
    }
    make.Add(BRepBuilderAPI_MakeEdge(source_last, offset_end).Edge());
    if (forward) {
        for (std::size_t index = offset_edges.size(); index-- > 0;) {
            make.Add(TopoDS::Edge(offset_edges[index].Reversed()));
        }
    }
    else {
        for (const auto& edge : offset_edges) {
            make.Add(edge);
        }
    }
    make.Add(BRepBuilderAPI_MakeEdge(offset_start, source_first).Edge());
    if (!make.IsDone()) {
        return TopoDS_Face();
    }

    BRepBuilderAPI_MakeFace face(make.Wire(), Standard_True);
    return face.IsDone() ? face.Face() : TopoDS_Face();
}

// The band faces between the open source wires and the open offset wires.
inline std::unique_ptr<TopoDS_Shape> fill_open(const std::vector<TopoDS_Wire>& source,
                                               const std::vector<TopoDS_Wire>& offset)
{
    std::vector<TopoDS_Wire> open_source;
    for (const auto& wire : source) {
        if (!BRep_Tool::IsClosed(wire)) {
            open_source.push_back(wire);
        }
    }
    std::vector<TopoDS_Wire> open_offset;
    for (const auto& wire : offset) {
        if (!BRep_Tool::IsClosed(wire)) {
            open_offset.push_back(wire);
        }
    }
    if (open_source.empty() || open_offset.empty()) {
        return nullptr;
    }

    std::vector<TopoDS_Shape> faces;
    std::vector<bool> used(open_offset.size(), false);
    for (const auto& source_wire : open_source) {
        std::vector<TopoDS_Edge> edges;
        collect_edges(source_wire, edges);
        TopoDS_Vertex first;
        TopoDS_Vertex last;
        if (!wire_ends(edges, first, last)) {
            continue;
        }
        const gp_Pnt point = BRep_Tool::Pnt(first);
        int best = -1;
        double best_distance = std::numeric_limits<double>::max();
        for (std::size_t index = 0; index < open_offset.size(); ++index) {
            if (used[index]) {
                continue;
            }
            std::vector<TopoDS_Edge> candidate_edges;
            collect_edges(open_offset[index], candidate_edges);
            TopoDS_Vertex candidate_first;
            TopoDS_Vertex candidate_last;
            if (!wire_ends(candidate_edges, candidate_first, candidate_last)) {
                continue;
            }
            const double distance = point.Distance(BRep_Tool::Pnt(candidate_first));
            if (distance < best_distance) {
                best = static_cast<int>(index);
                best_distance = distance;
            }
        }
        if (best < 0) {
            continue;
        }
        used[static_cast<std::size_t>(best)] = true;
        const TopoDS_Face band = open_band(source_wire, open_offset[static_cast<std::size_t>(best)]);
        if (band.IsNull()) {
            return nullptr;
        }
        faces.push_back(band);
    }
    return compound_of(faces);
}

// The wires the source shape is offset from. A face contributes its wires, a
// wire or edge itself, and a compound its children.
inline bool collect_source_wires(const TopoDS_Shape& shape, std::vector<TopoDS_Wire>& wires,
                                 bool& have_faces, bool& have_wires)
{
    switch (shape.ShapeType()) {
        case TopAbs_FACE: {
            have_faces = true;
            for (TopExp_Explorer explorer(shape, TopAbs_WIRE); explorer.More(); explorer.Next()) {
                wires.push_back(TopoDS::Wire(explorer.Current()));
            }
            return !wires.empty();
        }
        case TopAbs_WIRE:
            have_wires = true;
            wires.push_back(TopoDS::Wire(shape));
            return true;
        case TopAbs_EDGE: {
            BRepBuilderAPI_MakeWire make(TopoDS::Edge(shape));
            if (!make.IsDone()) {
                return false;
            }
            have_wires = true;
            wires.push_back(make.Wire());
            return true;
        }
        case TopAbs_COMPOUND: {
            bool any = false;
            for (TopoDS_Iterator iterator(shape); iterator.More(); iterator.Next()) {
                if (!collect_source_wires(iterator.Value(), wires, have_faces, have_wires)) {
                    return false;
                }
                any = true;
            }
            return any;
        }
        default:
            return false;
    }
}
}  // namespace fc_surface_ops_internal

// --- Partial spheres -------------------------------------------------------

// A sphere (or a slice of one) of `radius` centred at `(x, y, z)`.
//
// `u1`/`u2` are the longitude range around the local +Z axis (degrees, measured
// from the local +X direction) and `v1`/`v2` the latitude range measured from
// the equator (`-90` is the south pole, `+90` the north pole). The full sphere
// is `u1 = 0, u2 = 360, v1 = -90, v2 = 90`.
//
// `BRepPrimAPI_MakeSphere` fixes the longitude sweep to start at u = 0, so the
// slice is built there and then rotated by `u1` about the same axis — exact,
// no approximation.
inline std::unique_ptr<TopoDS_Shape> fc_make_sphere_slice(double radius, double x, double y,
                                                          double z, double u1, double u2,
                                                          double v1, double v2)
{
    using namespace fc_surface_ops_internal;
    if (!std::isfinite(radius) || radius <= Precision::Confusion()) {
        return nullptr;
    }
    if (!std::isfinite(u1) || !std::isfinite(u2) || !std::isfinite(v1) || !std::isfinite(v2)) {
        return nullptr;
    }
    if (u2 - u1 <= Precision::Confusion() || u2 - u1 > 360.0 + 1e-7) {
        return nullptr;
    }
    if (v2 - v1 <= Precision::Confusion() || v1 < -90.0 - 1e-7 || v2 > 90.0 + 1e-7) {
        return nullptr;
    }
    try {
        const gp_Ax2 axes(gp_Pnt(x, y, z), gp_Dir(0.0, 0.0, 1.0));
        BRepPrimAPI_MakeSphere make(axes, radius, radians(v1), radians(v2), radians(u2 - u1));
        TopoDS_Shape shape = make.Shape();
        if (shape.IsNull()) {
            return nullptr;
        }
        if (std::abs(u1) > 1e-12) {
            gp_Trsf rotation;
            rotation.SetRotation(gp_Ax1(gp_Pnt(x, y, z), gp_Dir(0.0, 0.0, 1.0)), radians(u1));
            BRepBuilderAPI_Transform transform(shape, rotation, true);
            if (!transform.IsDone()) {
                return nullptr;
            }
            shape = transform.Shape();
        }
        return owned(shape);
    }
    catch (...) {
        return nullptr;
    }
}

// --- Variable-radius fillet ------------------------------------------------

// Round `edges` of `shape`, with a linear radius law from `start_radii[i]` at
// the edge's first vertex to `end_radii[i]` at its last (upstream's
// `BRepFilletAPI_MakeFillet::Add(R1, R2, edge)`).
inline std::unique_ptr<TopoDS_Shape> fc_fillet_variable(
  const TopoDS_Shape& shape, const TopTools_ListOfShape& edges,
  rust::Slice<const double> start_radii, rust::Slice<const double> end_radii)
{
    using namespace fc_surface_ops_internal;
    if (shape.IsNull() || edges.IsEmpty()) {
        return nullptr;
    }
    const std::size_t count = static_cast<std::size_t>(edges.Size());
    if (start_radii.size() != count || end_radii.size() != count) {
        return nullptr;
    }
    try {
        BRepFilletAPI_MakeFillet make(shape);
        std::size_t index = 0;
        for (TopTools_ListIteratorOfListOfShape iterator(edges); iterator.More();
             iterator.Next(), ++index) {
            const TopoDS_Shape& edge = iterator.Value();
            if (edge.IsNull() || edge.ShapeType() != TopAbs_EDGE) {
                return nullptr;
            }
            const double start = start_radii[index];
            const double end = end_radii[index];
            if (!std::isfinite(start) || !std::isfinite(end) || start <= Precision::Confusion()
                || end <= Precision::Confusion()) {
                return nullptr;
            }
            if (std::abs(start - end) <= Precision::Confusion()) {
                make.Add(start, TopoDS::Edge(edge));
            }
            else {
                make.Add(start, end, TopoDS::Edge(edge));
            }
        }
        make.Build();
        if (!make.IsDone()) {
            return nullptr;
        }
        return owned(make.Shape());
    }
    catch (...) {
        return nullptr;
    }
}

// --- Planar offset with filling --------------------------------------------

// Offset `shape` by `distance` in its plane (`BRepOffsetAPI_MakeOffset`).
//
// `kind` is upstream's `JoinType` (0 arc, 1 tangent, 2 intersection). With
// `fill = true` the result is the face between the source and its offset
// (upstream's fill), not the offset boundary: closed wires become bands by
// nesting, and open wires are closed with two straight connectors when
// `open_result` is set. Without `fill`, a face input is rebuilt from the offset
// wires and a wire/edge input returns the offset boundary.
inline std::unique_ptr<TopoDS_Shape> fc_offset2d(const TopoDS_Shape& shape, double distance,
                                                 int kind, bool fill, bool open_result)
{
    using namespace fc_surface_ops_internal;
    if (shape.IsNull() || !std::isfinite(distance)) {
        return nullptr;
    }
    if (fill && std::abs(distance) <= Precision::Confusion()) {
        return nullptr;
    }
    try {
        std::vector<TopoDS_Wire> source_wires;
        bool have_faces = false;
        bool have_wires = false;
        if (!collect_source_wires(shape, source_wires, have_faces, have_wires)
            || source_wires.empty()) {
            return nullptr;
        }
        if (have_faces && have_wires) {
            return nullptr;
        }

        BRepOffsetAPI_MakeOffset make;
        make.Init(join_type(kind), open_result ? Standard_True : Standard_False);
        for (const auto& wire : source_wires) {
            make.AddWire(wire);
        }
        make.Perform(distance, 0.0);
        if (!make.IsDone()) {
            return nullptr;
        }
        const TopoDS_Shape offset = make.Shape();
        if (offset.IsNull()) {
            return nullptr;
        }

        std::vector<TopoDS_Wire> offset_wires;
        for (TopExp_Explorer explorer(offset, TopAbs_WIRE); explorer.More(); explorer.Next()) {
            offset_wires.push_back(TopoDS::Wire(explorer.Current()));
        }
        if (offset_wires.empty()) {
            return nullptr;
        }

        if (!fill) {
            if (!have_faces) {
                return owned(offset);
            }
            return filled_regions(offset_wires);
        }

        std::vector<TopoDS_Shape> faces;
        if (auto closed = fill_closed(source_wires, offset_wires)) {
            for (TopExp_Explorer explorer(*closed, TopAbs_FACE); explorer.More();
                 explorer.Next()) {
                faces.push_back(explorer.Current());
            }
        }
        if (auto open = fill_open(source_wires, offset_wires)) {
            for (TopExp_Explorer explorer(*open, TopAbs_FACE); explorer.More();
                 explorer.Next()) {
                faces.push_back(explorer.Current());
            }
        }
        return compound_of(faces);
    }
    catch (...) {
        return nullptr;
    }
}

// --- Solids with cavities --------------------------------------------------

// A solid from `shells`: the first is the outer skin and every later shell is
// a cavity (taken outer-oriented, as the primitives build it, and reversed
// here so the cavity removes material).
inline std::unique_ptr<TopoDS_Shape> fc_make_hollow_solid(
  const TopTools_ListOfShape& shells)
{
    using namespace fc_surface_ops_internal;
    if (shells.IsEmpty()) {
        return nullptr;
    }
    try {
        TopTools_ListIteratorOfListOfShape iterator(shells);
        if (iterator.Value().IsNull() || iterator.Value().ShapeType() != TopAbs_SHELL) {
            return nullptr;
        }
        BRepBuilderAPI_MakeSolid make(TopoDS::Shell(iterator.Value()));
        for (iterator.Next(); iterator.More(); iterator.Next()) {
            if (iterator.Value().IsNull() || iterator.Value().ShapeType() != TopAbs_SHELL) {
                return nullptr;
            }
            make.Add(TopoDS::Shell(iterator.Value().Reversed()));
        }
        if (!make.IsDone()) {
            return nullptr;
        }
        TopoDS_Solid solid = make.Solid();
        BRepLib::OrientClosedSolid(solid);
        return owned(solid);
    }
    catch (...) {
        return nullptr;
    }
}

// --- Exact-by-construction helix -------------------------------------------

// The circular or conical helix of `pitch`, `height` and base `radius` about
// +Z, as one edge whose pcurve is a straight line in the (u, v) parameter
// space of an analytic cylinder/cone (`Geom_CylindricalSurface` /
// `Geom_ConicalSurface`).
//
// A circular helix is transcendental and cannot be a polynomial curve; this is
// the exact construction upstream uses: the edge lies exactly on the analytic
// surface (the pcurve is exact), and `BRepLib::BuildCurves3d` builds the 3D
// representative curve to its tolerance. `angle` is the cone's half angle in
// degrees (0 or negative: a cylinder) and the conical height is measured along
// the axis, so the radius grows by `height * tan(angle)`.
inline std::unique_ptr<TopoDS_Shape> fc_make_helix_edge(double pitch, double height,
                                                        double radius, double angle,
                                                        bool left_handed)
{
    using namespace fc_surface_ops_internal;
    if (!std::isfinite(pitch) || std::abs(pitch) <= Precision::Confusion()) {
        return nullptr;
    }
    if (!std::isfinite(height) || std::abs(height) <= Precision::Confusion()) {
        return nullptr;
    }
    if ((height > 0.0 && pitch < 0.0) || (height < 0.0 && pitch > 0.0)) {
        return nullptr;
    }
    if (!std::isfinite(angle)) {
        return nullptr;
    }
    const bool conical = angle > 1e-9;
    double semi_angle = 0.0;
    if (conical) {
        semi_angle = radians(angle);
        if (semi_angle <= Precision::Confusion() || semi_angle >= kPi * 0.5 - 1e-12) {
            return nullptr;
        }
    }
    else if (radius <= Precision::Confusion()) {
        return nullptr;
    }
    if (!std::isfinite(radius)) {
        return nullptr;
    }
    try {
        const gp_Ax2 axis(gp_Pnt(0.0, 0.0, 0.0), gp_Dir(0.0, 0.0, 1.0));
        Handle(Geom_Surface) surface;
        if (conical) {
            surface = new Geom_ConicalSurface(gp_Ax3(axis), semi_angle, radius);
        }
        else {
            surface = new Geom_CylindricalSurface(axis, radius);
        }

        const double direction = left_handed ? -1.0 : 1.0;
        const gp_Pnt2d start(0.0, 0.0);
        const double u = direction * kTwoPi * (height / pitch);
        const double v = conical ? height / std::cos(semi_angle) : height;
        Handle(Geom2d_TrimmedCurve) segment = GCE2d_MakeSegment(start, gp_Pnt2d(u, v));
        const TopoDS_Edge edge = BRepBuilderAPI_MakeEdge(segment, surface);
        if (edge.IsNull()) {
            return nullptr;
        }
        BRepLib::BuildCurves3d(edge);
        return owned(edge);
    }
    catch (...) {
        return nullptr;
    }
}

// --- Pipe shell ------------------------------------------------------------

// Sweep `profiles` along `spine` with `BRepOffsetAPI_MakePipeShell`.
//
// `spine` may be a wire or a single edge. `frenet` selects a Frenet trihedron
// instead of the corrected Frenet one (`SetMode`); the transition is OCCT's
// `BRepBuilderAPI_Transformed`, the default for a smooth multi-section sweep.
// With `solid` the swept shell is closed into a solid (`MakeSolid`).
inline std::unique_ptr<TopoDS_Shape> fc_make_pipe_shell(const TopoDS_Shape& spine,
                                                        const TopTools_ListOfShape& profiles,
                                                        bool solid, bool frenet)
{
    using namespace fc_surface_ops_internal;
    if (spine.IsNull() || profiles.IsEmpty()) {
        return nullptr;
    }
    try {
        TopoDS_Wire wire;
        if (spine.ShapeType() == TopAbs_WIRE) {
            wire = TopoDS::Wire(spine);
        }
        else if (spine.ShapeType() == TopAbs_EDGE) {
            BRepBuilderAPI_MakeWire make(TopoDS::Edge(spine));
            if (!make.IsDone()) {
                return nullptr;
            }
            wire = make.Wire();
        }
        else {
            return nullptr;
        }

        BRepOffsetAPI_MakePipeShell make(wire);
        make.SetMode(frenet ? Standard_True : Standard_False);
        make.SetTransitionMode(BRepBuilderAPI_Transformed);
        for (TopTools_ListIteratorOfListOfShape iterator(profiles); iterator.More();
             iterator.Next()) {
            const TopoDS_Shape& profile = iterator.Value();
            if (profile.IsNull()) {
                return nullptr;
            }
            // `Add` takes a wire or a vertex: an edge is wrapped in a wire and
            // a face contributes its outer wire, as upstream's
            // `prepareProfiles` does.
            if (profile.ShapeType() == TopAbs_WIRE || profile.ShapeType() == TopAbs_VERTEX) {
                make.Add(profile);
            }
            else if (profile.ShapeType() == TopAbs_EDGE) {
                BRepBuilderAPI_MakeWire profile_wire(TopoDS::Edge(profile));
                if (!profile_wire.IsDone()) {
                    return nullptr;
                }
                make.Add(profile_wire.Wire());
            }
            else if (profile.ShapeType() == TopAbs_FACE) {
                TopExp_Explorer explorer(profile, TopAbs_WIRE);
                if (!explorer.More()) {
                    return nullptr;
                }
                make.Add(TopoDS::Wire(explorer.Current()));
            }
            else {
                return nullptr;
            }
        }
        if (!make.IsReady()) {
            return nullptr;
        }
        make.Build();
        if (solid && !make.MakeSolid()) {
            return nullptr;
        }
        TopoDS_Shape result = make.Shape();
        if (result.IsNull()) {
            return nullptr;
        }
        // The builder may wrap the solid in a one-child compound; return the
        // solid itself so `Shape.ShapeType` is stable.
        if (solid && result.ShapeType() == TopAbs_COMPOUND) {
            TopoDS_Iterator iterator(result);
            const TopoDS_Shape first = iterator.More() ? iterator.Value() : TopoDS_Shape();
            iterator.Next();
            if (!first.IsNull() && !iterator.More() && first.ShapeType() == TopAbs_SOLID) {
                result = first;
            }
        }
        return owned(result);
    }
    catch (...) {
        return nullptr;
    }
}
