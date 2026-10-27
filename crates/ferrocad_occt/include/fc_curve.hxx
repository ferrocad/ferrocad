#pragma once

// FerroCAD: the analytic curve behind an edge (`edge.Curve` upstream).
//
// `opencascade-sys` 0.3 bridges `BRep_Tool::Curve` but not the curve's dynamic
// type or a `Geom_*`'s parameters, so the classification lives here. Companion to
// `fc_history.hxx` / `fc_topology.hxx`; see `docs/tasks/p2-architecture.md` P2.2.
//
// The result is one flat `rust::Vec<double>`. Every payload starts with the same
// 14-value header, so the previous line/circle/ellipse format is unchanged:
//
//   [kind, ox, oy, oz, dx, dy, dz, xx, xy, xz, first, last, major, minor, tail…]
//
// kind: 0 = unsupported, 1 = line, 2 = circle/arc, 3 = ellipse/elliptical arc,
//       4 = hyperbola/arc, 5 = parabola/arc, 6 = Bezier, 7 = B-spline,
//       8 = offset curve.
//
// `o` is the line origin / conic centre / parabola vertex; `d` the line direction,
// the plane normal or the offset direction; `x` the circle/ellipse x axis or the
// conic's symmetry axis; `first`/`last` the edge's trimmed parameter range;
// `major`/`minor` the conic radii (parabola: `major` is the focal length; offset:
// `major` is the signed offset). On kind 0 every other value is 0.
//
// The tail holds what does not fit the header:
//   kind 6: [pole_count], pole_count*3 coordinates, pole_count weights.
//   kind 7: [degree, pole_count, knot_count, mult_count, periodic],
//           pole_count*3 coordinates, pole_count weights, knot_count knots,
//           mult_count multiplicities. `periodic` is 1 when OCCT reports a
//           periodic `Geom_BSplineCurve`; its knots still carry the period-end
//           knot, so `sum(mults) = pole_count + mult[0]` rather than the clamped
//           `pole_count + degree + 1`. A trimmed edge of a periodic curve is
//           segmented first, so it reports the (clamped) segment.
//   kind 8: [basis_length] followed by a complete nested payload of that length.
//
// An edge's `BRep_Tool::Curve` may wrap the basis in `Geom_TrimmedCurve`; the
// basis is classified and `first`/`last` come from the trim.

#include <BOPAlgo_BuilderFace.hxx>
#include <BRepAdaptor_Curve.hxx>
#include <BRepAlgoAPI_BuilderAlgo.hxx>
#include <BRepAlgoAPI_Section.hxx>
#include <BRepBuilderAPI_Copy.hxx>
#include <BRepBuilderAPI_MakeEdge.hxx>
#include <BRepBuilderAPI_MakeFace.hxx>
#include <BRepBuilderAPI_MakeWire.hxx>
#include <BRepCheck_Analyzer.hxx>
#include <BRepFeat_SplitShape.hxx>
#include <BRepOffsetAPI_MakeOffset.hxx>
#include <BRepOffsetAPI_MakePipe.hxx>
#include <BRepOffsetAPI_MakeThickSolid.hxx>
#include <BRepTools_ReShape.hxx>
#include <BRepFill.hxx>
#include <BRepFill_Filling.hxx>
#include <BRepLib.hxx>
#include <BRepLib_FindSurface.hxx>
#include <Bnd_Box.hxx>
#include <BRepBndLib.hxx>
#include <BRepGProp.hxx>
#include <BRep_Builder.hxx>
#include <TopoDS_CompSolid.hxx>
#include <TopoDS_Compound.hxx>
#include <BRep_Tool.hxx>
#include <BSplCLib.hxx>
#include <Geom2dAPI_InterCurveCurve.hxx>
#include <Geom2dAPI_ProjectPointOnCurve.hxx>
#include <Geom2d_Curve.hxx>
#include <GeomAPI.hxx>
#include <GeomAPI_Interpolate.hxx>
#include <GeomAdaptor_Surface.hxx>
#include <GeomConvert.hxx>
#include <Geom_BSplineCurve.hxx>
#include <Geom_Conic.hxx>
#include <GProp_GProps.hxx>
#include <GProp_PrincipalProps.hxx>
#include <Precision.hxx>
#include <TColStd_Array1OfInteger.hxx>
#include <TColStd_Array1OfReal.hxx>
#include <TColgp_Array1OfPnt.hxx>
#include <TColgp_HArray1OfPnt.hxx>
#include <GeomAbs_JoinType.hxx>
#include <GeomAbs_Shape.hxx>
#include <Geom_Hyperbola.hxx>
#include <Geom_Parabola.hxx>
#include <Geom_BezierCurve.hxx>
#include <Geom_OffsetCurve.hxx>
#include <gp_Hypr.hxx>
#include <gp_Parab.hxx>
#include <Message_ProgressRange.hxx>
#include <Geom_Curve.hxx>
#include <Geom_Circle.hxx>
#include <Geom_Ellipse.hxx>
#include <Geom_Line.hxx>
#include <Geom_TrimmedCurve.hxx>
#include <ShapeConstruct_Curve.hxx>
#include <Standard_Type.hxx>
#include <gp_Pln.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Edge.hxx>
#include <TopoDS_Face.hxx>
#include <TopoDS_Shape.hxx>
#include <TopoDS_Shell.hxx>
#include <TopoDS_Vertex.hxx>
#include <TopExp_Explorer.hxx>
#include <TopoDS_Wire.hxx>
#include <TopTools_ListIteratorOfListOfShape.hxx>
#include <TopTools_ListOfShape.hxx>
#include <gp_Ax1.hxx>
#include <gp_Ax2.hxx>
#include <gp_Circ.hxx>
#include <gp_Dir.hxx>
#include <gp_Elips.hxx>
#include <gp_Lin.hxx>
#include <gp_Pnt.hxx>
#include <gp_Trsf.hxx>

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <limits>
#include <memory>
#include <vector>

#include "rust/cxx.h"

inline Handle(Geom_BSplineCurve) fc_bspline_curve(rust::Slice<const double> coords,
                                                 rust::Slice<const double> weights,
                                                 rust::Slice<const double> knots,
                                                 rust::Slice<const int32_t> multiplicities,
                                                 int degree,
                                                 bool periodic,
                                                 bool check_rational);

inline void fc_curve_push_point(rust::Vec<double>& out, const gp_Pnt& point)
{
  out.push_back(point.X());
  out.push_back(point.Y());
  out.push_back(point.Z());
}

inline void fc_curve_push_dir(rust::Vec<double>& out, const gp_Dir& direction)
{
  out.push_back(direction.X());
  out.push_back(direction.Y());
  out.push_back(direction.Z());
}

inline void fc_curve_push_header(rust::Vec<double>& out, double first, double last)
{
  const std::size_t at = out.size();
  for (int i = 0; i < 14; ++i) {
    out.push_back(0.0);
  }
  out[at + 10] = first;
  out[at + 11] = last;
}

inline void fc_curve_append(rust::Vec<double>& out,
                            const Handle(Geom_Curve)& input,
                            double first,
                            double last,
                            int depth)
{
  fc_curve_push_header(out, first, last);
  const std::size_t at = out.size() - 14;
  if (input.IsNull() || depth > 8) {
    return;
  }

  Handle(Geom_Curve) curve = input;
  if (curve->IsKind(STANDARD_TYPE(Geom_TrimmedCurve))) {
    Handle(Geom_TrimmedCurve) trimmed = Handle(Geom_TrimmedCurve)::DownCast(curve);
    if (!trimmed.IsNull()) {
      first = trimmed->FirstParameter();
      last = trimmed->LastParameter();
      out[at + 10] = first;
      out[at + 11] = last;
      curve = trimmed->BasisCurve();
    }
  }
  if (curve.IsNull()) {
    return;
  }

  if (curve->IsKind(STANDARD_TYPE(Geom_Line))) {
    Handle(Geom_Line) line = Handle(Geom_Line)::DownCast(curve);
    if (line.IsNull()) {
      return;
    }
    const gp_Lin& lin = line->Lin();
    out[at] = 1.0;
    out[at + 1] = lin.Location().X();
    out[at + 2] = lin.Location().Y();
    out[at + 3] = lin.Location().Z();
    out[at + 4] = lin.Direction().X();
    out[at + 5] = lin.Direction().Y();
    out[at + 6] = lin.Direction().Z();
    return;
  }

  if (curve->IsKind(STANDARD_TYPE(Geom_Circle))) {
    Handle(Geom_Circle) circle = Handle(Geom_Circle)::DownCast(curve);
    if (circle.IsNull()) {
      return;
    }
    const gp_Circ& circ = circle->Circ();
    const gp_Ax2& position = circ.Position();
    out[at] = 2.0;
    out[at + 1] = position.Location().X();
    out[at + 2] = position.Location().Y();
    out[at + 3] = position.Location().Z();
    out[at + 4] = position.Direction().X();
    out[at + 5] = position.Direction().Y();
    out[at + 6] = position.Direction().Z();
    out[at + 7] = position.XDirection().X();
    out[at + 8] = position.XDirection().Y();
    out[at + 9] = position.XDirection().Z();
    out[at + 12] = circ.Radius();
    return;
  }

  if (curve->IsKind(STANDARD_TYPE(Geom_Ellipse))) {
    Handle(Geom_Ellipse) ellipse = Handle(Geom_Ellipse)::DownCast(curve);
    if (ellipse.IsNull()) {
      return;
    }
    const gp_Elips& elips = ellipse->Elips();
    const gp_Ax2& position = elips.Position();
    out[at] = 3.0;
    out[at + 1] = position.Location().X();
    out[at + 2] = position.Location().Y();
    out[at + 3] = position.Location().Z();
    out[at + 4] = position.Direction().X();
    out[at + 5] = position.Direction().Y();
    out[at + 6] = position.Direction().Z();
    out[at + 7] = position.XDirection().X();
    out[at + 8] = position.XDirection().Y();
    out[at + 9] = position.XDirection().Z();
    out[at + 12] = elips.MajorRadius();
    out[at + 13] = elips.MinorRadius();
    return;
  }

  if (curve->IsKind(STANDARD_TYPE(Geom_Hyperbola))) {
    Handle(Geom_Hyperbola) hyperbola = Handle(Geom_Hyperbola)::DownCast(curve);
    if (hyperbola.IsNull()) {
      return;
    }
    const gp_Hypr& hypr = hyperbola->Hypr();
    const gp_Ax2& position = hypr.Position();
    out[at] = 4.0;
    out[at + 1] = position.Location().X();
    out[at + 2] = position.Location().Y();
    out[at + 3] = position.Location().Z();
    out[at + 4] = position.Direction().X();
    out[at + 5] = position.Direction().Y();
    out[at + 6] = position.Direction().Z();
    out[at + 7] = position.XDirection().X();
    out[at + 8] = position.XDirection().Y();
    out[at + 9] = position.XDirection().Z();
    out[at + 12] = hypr.MajorRadius();
    out[at + 13] = hypr.MinorRadius();
    return;
  }

  if (curve->IsKind(STANDARD_TYPE(Geom_Parabola))) {
    Handle(Geom_Parabola) parabola = Handle(Geom_Parabola)::DownCast(curve);
    if (parabola.IsNull()) {
      return;
    }
    const gp_Parab& parab = parabola->Parab();
    const gp_Ax2& position = parab.Position();
    out[at] = 5.0;
    out[at + 1] = position.Location().X();
    out[at + 2] = position.Location().Y();
    out[at + 3] = position.Location().Z();
    out[at + 4] = position.Direction().X();
    out[at + 5] = position.Direction().Y();
    out[at + 6] = position.Direction().Z();
    out[at + 7] = position.XDirection().X();
    out[at + 8] = position.XDirection().Y();
    out[at + 9] = position.XDirection().Z();
    out[at + 12] = parab.Focal();
    return;
  }

  if (curve->IsKind(STANDARD_TYPE(Geom_BezierCurve))) {
    Handle(Geom_BezierCurve) bezier = Handle(Geom_BezierCurve)::DownCast(curve);
    if (bezier.IsNull()) {
      return;
    }
    out[at] = 6.0;
    const Standard_Integer count = bezier->NbPoles();
    out.push_back(static_cast<double>(count));
    for (Standard_Integer i = 1; i <= count; ++i) {
      fc_curve_push_point(out, bezier->Pole(i));
    }
    for (Standard_Integer i = 1; i <= count; ++i) {
      out.push_back(bezier->Weight(i));
    }
    return;
  }

  if (curve->IsKind(STANDARD_TYPE(Geom_BSplineCurve))) {
    Handle(Geom_BSplineCurve) spline = Handle(Geom_BSplineCurve)::DownCast(curve);
    if (spline.IsNull()) {
      return;
    }
    // A B-spline edge may trim a wider knot domain without a wrapper; re-segment
    // so the bounds survive in the knot vector itself. `Segment` returns a
    // non-periodic curve, so a periodic curve is only reported when the edge
    // covers the whole period.
    const double domain_first = spline->FirstParameter();
    const double domain_last = spline->LastParameter();
    if (first > domain_first + 1e-12 || last < domain_last - 1e-12) {
      Handle(Geom_BSplineCurve) segment =
          Handle(Geom_BSplineCurve)::DownCast(spline->Copy());
      if (segment.IsNull()) {
        return;
      }
      segment->Segment(first, last);
      spline = segment;
      first = spline->FirstParameter();
      last = spline->LastParameter();
      out[at + 10] = first;
      out[at + 11] = last;
    }
    out[at] = 7.0;
    const Standard_Integer degree = spline->Degree();
    const Standard_Integer pole_count = spline->NbPoles();
    const Standard_Integer knot_count = spline->NbKnots();
    out.push_back(static_cast<double>(degree));
    out.push_back(static_cast<double>(pole_count));
    out.push_back(static_cast<double>(knot_count));
    out.push_back(static_cast<double>(knot_count));
    out.push_back(spline->IsPeriodic() ? 1.0 : 0.0);
    for (Standard_Integer i = 1; i <= pole_count; ++i) {
      fc_curve_push_point(out, spline->Pole(i));
    }
    for (Standard_Integer i = 1; i <= pole_count; ++i) {
      out.push_back(spline->Weight(i));
    }
    for (Standard_Integer i = 1; i <= knot_count; ++i) {
      out.push_back(spline->Knot(i));
    }
    for (Standard_Integer i = 1; i <= knot_count; ++i) {
      out.push_back(static_cast<double>(spline->Multiplicity(i)));
    }
    return;
  }

  if (curve->IsKind(STANDARD_TYPE(Geom_OffsetCurve))) {
    Handle(Geom_OffsetCurve) offset = Handle(Geom_OffsetCurve)::DownCast(curve);
    if (offset.IsNull()) {
      return;
    }
    const gp_Dir& direction = offset->Direction();
    out[at] = 8.0;
    out[at + 4] = direction.X();
    out[at + 5] = direction.Y();
    out[at + 6] = direction.Z();
    out[at + 12] = offset->Offset();
    const std::size_t length_at = out.size();
    out.push_back(0.0);
    const Handle(Geom_Curve)& basis = offset->BasisCurve();
    fc_curve_append(out,
                    basis,
                    basis.IsNull() ? first : basis->FirstParameter(),
                    basis.IsNull() ? last : basis->LastParameter(),
                    depth + 1);
    out[length_at] = static_cast<double>(out.size() - length_at - 1);
    return;
  }
}

inline rust::Vec<double> fc_edge_curve(const TopoDS_Shape& shape)
{
  rust::Vec<double> out;
  out.reserve(64);

  if (shape.IsNull() || shape.ShapeType() != TopAbs_EDGE) {
    fc_curve_push_header(out, 0.0, 0.0);
    return out;
  }
  const TopoDS_Edge& edge = TopoDS::Edge(shape);
  double first = 0.0;
  double last = 0.0;
  Handle(Geom_Curve) curve = BRep_Tool::Curve(edge, first, last);
  try {
    fc_curve_append(out, curve, first, last, 0);
  }
  catch (...) {
    out.clear();
    fc_curve_push_header(out, 0.0, 0.0);
  }
  return out;
}

// A trimmed conic edge: kind 3 = ellipse, 4 = hyperbola, 5 = parabola. The frame
// is passed explicitly so the caller keeps its own possibly rotated axes.
// Returns null on failure.
inline std::unique_ptr<TopoDS_Shape> fc_conic_edge(int kind,
                                                   double ox,
                                                   double oy,
                                                   double oz,
                                                   double nx,
                                                   double ny,
                                                   double nz,
                                                   double xx,
                                                   double xy,
                                                   double xz,
                                                   double major,
                                                   double minor,
                                                   double first,
                                                   double last)
{
  try {
    const gp_Ax2 position(gp_Pnt(ox, oy, oz), gp_Dir(nx, ny, nz), gp_Dir(xx, xy, xz));
    Handle(Geom_Curve) curve;
    if (kind == 3) {
      curve = new Geom_Ellipse(gp_Elips(position, major, minor));
    }
    else if (kind == 4) {
      curve = new Geom_Hyperbola(gp_Hypr(position, major, minor));
    }
    else if (kind == 5) {
      curve = new Geom_Parabola(gp_Parab(position, major));
    }
    else {
      return nullptr;
    }
    BRepBuilderAPI_MakeEdge make(curve, first, last);
    if (!make.IsDone()) {
      return nullptr;
    }
    return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(make.Edge()));
  }
  catch (...) {
    return nullptr;
  }
}

// A Bezier edge from explicit poles/weights over the curve's natural [0, 1]
// parameter range (`Geom_BezierCurve` is always parameterised on [0, 1]). Returns
// null on failure.
inline std::unique_ptr<TopoDS_Shape> fc_bezier_edge(rust::Slice<const double> coords,
                                                    rust::Slice<const double> weights)
{
  const int count = static_cast<int>(coords.size() / 3);
  if (count < 2 || static_cast<int>(weights.size()) != count) {
    return nullptr;
  }
  try {
    TColgp_Array1OfPnt poles(1, count);
    bool rational = false;
    for (int i = 0; i < count; ++i) {
      poles.SetValue(i + 1, gp_Pnt(coords[3 * i], coords[3 * i + 1], coords[3 * i + 2]));
      rational = rational || std::abs(weights[i] - 1.0) > 1e-12;
    }
    Handle(Geom_BezierCurve) curve;
    if (rational) {
      TColStd_Array1OfReal values(1, count);
      for (int i = 0; i < count; ++i) {
        values.SetValue(i + 1, weights[i]);
      }
      curve = new Geom_BezierCurve(poles, values);
    }
    else {
      curve = new Geom_BezierCurve(poles);
    }
    BRepBuilderAPI_MakeEdge make(curve);
    if (!make.IsDone()) {
      return nullptr;
    }
    return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(make.Edge()));
  }
  catch (...) {
    return nullptr;
  }
}

// A B-spline edge over `[first, last]`, re-segmenting the curve when the range is
// narrower than its knot domain. A periodic curve keeps its periodic form when
// the edge covers the whole period. Returns null on failure.
inline std::unique_ptr<TopoDS_Shape> fc_bspline_edge_range(rust::Slice<const double> coords,
                                                           rust::Slice<const double> weights,
                                                           rust::Slice<const double> knots,
                                                           rust::Slice<const int32_t> multiplicities,
                                                           int degree,
                                                           bool periodic,
                                                           double first,
                                                           double last)
{
  try {
    Handle(Geom_BSplineCurve) curve =
        fc_bspline_curve(coords, weights, knots, multiplicities, degree, periodic, false);
    if (curve.IsNull()) {
      return nullptr;
    }
    if (first > curve->FirstParameter() + 1e-12 || last < curve->LastParameter() - 1e-12) {
      Handle(Geom_BSplineCurve) segment = Handle(Geom_BSplineCurve)::DownCast(curve->Copy());
      if (segment.IsNull()) {
        return nullptr;
      }
      segment->Segment(first, last);
      curve = segment;
    }
    BRepBuilderAPI_MakeEdge make(curve);
    if (!make.IsDone()) {
      return nullptr;
    }
    return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(make.Edge()));
  }
  catch (...) {
    return nullptr;
  }
}

// An edge of the offset curve of `basis` over `[first, last]`. The basis' own
// curve (and its parameterisation) is read back from the edge. Returns null on
// failure.
inline std::unique_ptr<TopoDS_Shape> fc_offset_curve_edge(const TopoDS_Shape& basis,
                                                          double offset,
                                                          double dx,
                                                          double dy,
                                                          double dz,
                                                          double first,
                                                          double last)
{
  try {
    if (basis.IsNull() || basis.ShapeType() != TopAbs_EDGE) {
      return nullptr;
    }
    double basis_first = 0.0;
    double basis_last = 0.0;
    Handle(Geom_Curve) base = BRep_Tool::Curve(TopoDS::Edge(basis), basis_first, basis_last);
    if (base.IsNull()) {
      return nullptr;
    }
    Handle(Geom_OffsetCurve) curve = new Geom_OffsetCurve(base, offset, gp_Dir(dx, dy, dz));
    BRepBuilderAPI_MakeEdge make(curve, first, last);
    if (!make.IsDone()) {
      return nullptr;
    }
    return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(make.Edge()));
  }
  catch (...) {
    return nullptr;
  }
}

// The edge view of a shape, for `BRepBuilderAPI_MakeWire::Add`.
inline const TopoDS_Edge& fc_shape_as_edge(const TopoDS_Shape& shape)
{
  return TopoDS::Edge(shape);
}

// The face view of a shape, for `BRep_Tool::Surface` and `BRepTools::OuterWire`.
inline const TopoDS_Face& fc_shape_as_face(const TopoDS_Shape& shape)
{
  return TopoDS::Face(shape);
}

// The vertex/wire views, for `BRep_Tool::Pnt` and `BRepBuilderAPI_MakeFace`.
inline const TopoDS_Vertex& fc_shape_as_vertex(const TopoDS_Shape& shape)
{
  return TopoDS::Vertex(shape);
}

inline const TopoDS_Wire& fc_shape_as_wire(const TopoDS_Shape& shape)
{
  return TopoDS::Wire(shape);
}

// The shell view of a shape, for `BRepBuilderAPI_MakeSolid`.
inline const TopoDS_Shell& fc_shape_as_shell(const TopoDS_Shape& shape)
{
  return TopoDS::Shell(shape);
}

// Shape-type guards, so a wrong-kind shape is reported instead of throwing
// `Standard_TypeMismatch` across the bridge.
inline bool fc_is_edge(const TopoDS_Shape& shape)
{
  return !shape.IsNull() && shape.ShapeType() == TopAbs_EDGE;
}

inline bool fc_is_wire(const TopoDS_Shape& shape)
{
  return !shape.IsNull() && shape.ShapeType() == TopAbs_WIRE;
}

inline bool fc_is_vertex(const TopoDS_Shape& shape)
{
  return !shape.IsNull() && shape.ShapeType() == TopAbs_VERTEX;
}

inline bool fc_is_face(const TopoDS_Shape& shape)
{
  return !shape.IsNull() && shape.ShapeType() == TopAbs_FACE;
}

inline bool fc_is_shell(const TopoDS_Shape& shape)
{
  return !shape.IsNull() && shape.ShapeType() == TopAbs_SHELL;
}

// A copy of `shape` with reversed orientation (a hole wire is opposite to its
// outer boundary).
inline std::unique_ptr<TopoDS_Shape> fc_reversed(const TopoDS_Shape& shape)
{
  std::unique_ptr<TopoDS_Shape> reversed(new TopoDS_Shape(shape));
  reversed->Reverse();
  return reversed;
}

// Install a row-major 4x4 matrix into an OCCT transform. The fourth row is
// ignored (OCCT transforms are affine).
inline bool fc_trsf_from_matrix(rust::Slice<const double> m, gp_Trsf& out)
{
  if (m.size() < 16) {
    return false;
  }
  try {
    out.SetValues(m[0], m[1], m[2], m[3],
                  m[4], m[5], m[6], m[7],
                  m[8], m[9], m[10], m[11]);
    return true;
  }
  catch (...) {
    return false;
  }
}

// A B-spline from control points (`Part.BSplineCurve(poles, ...)`). `coords` is
// a flat xyz list. Returns null on failure (including an OCCT exception).
inline std::unique_ptr<Handle_Geom_Curve> fc_bspline_from_poles(rust::Slice<const double> coords,
                                                               int degree,
                                                               bool periodic)
{
  const int count = static_cast<int>(coords.size() / 3);
  // A periodic B-spline needs a different knot structure; this slice only builds
  // the clamped (non-periodic) form.
  if (count < 2 || degree < 1 || periodic || count <= degree) {
    return nullptr;
  }
  try {
    TColgp_Array1OfPnt poles(1, count);
    for (int i = 0; i < count; ++i) {
      poles.SetValue(i + 1, gp_Pnt(coords[3 * i], coords[3 * i + 1], coords[3 * i + 2]));
    }
    const int distinct = count - degree + 1;
    TColStd_Array1OfReal knots(1, distinct);
    TColStd_Array1OfInteger multiplicities(1, distinct);
    for (int i = 1; i <= distinct; ++i) {
      knots.SetValue(i, static_cast<double>(i - 1));
      multiplicities.SetValue(i, i == 1 || i == distinct ? degree + 1 : 1);
    }
    Handle(Geom_BSplineCurve) curve =
        new Geom_BSplineCurve(poles, knots, multiplicities, degree, Standard_False);
    return std::unique_ptr<Handle_Geom_Curve>(new Handle_Geom_Curve(curve));
  }
  catch (...) {
    return nullptr;
  }
}

// A full elliptical edge (`Part.Ellipse.toShape`). The frame is passed explicitly
// (centre, normal, major-axis direction, the two radii) so the caller keeps its
// own possibly rotated frame; `gp_Ax2` re-orthogonalises the x direction.
// Returns null on failure.
inline std::unique_ptr<TopoDS_Shape> fc_ellipse_edge(double ox,
                                                   double oy,
                                                   double oz,
                                                   double nx,
                                                   double ny,
                                                   double nz,
                                                   double xx,
                                                   double xy,
                                                   double xz,
                                                   double major,
                                                   double minor)
{
  try {
    const gp_Ax2 position(gp_Pnt(ox, oy, oz), gp_Dir(nx, ny, nz), gp_Dir(xx, xy, xz));
    const gp_Elips elips(position, major, minor);
    Handle(Geom_Ellipse) curve = new Geom_Ellipse(elips);
    BRepBuilderAPI_MakeEdge make(curve);
    if (!make.IsDone()) {
      return nullptr;
    }
    return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(make.Edge()));
  }
  catch (...) {
    return nullptr;
  }
}

// The general B-spline form behind `Part.BSplineCurve.buildFromPolesMultsKnots`
// (and its constructor): explicit poles, weights, unique knots and
// multiplicities. `fc_bspline_from_poles` above is the uniform shortcut.
// `coords` is a flat xyz list, `weights` one value per pole, and `multiplicities`
// one value per knot. Returns a null handle on failure (including an OCCT
// exception).
inline Handle(Geom_BSplineCurve) fc_bspline_curve(rust::Slice<const double> coords,
                                                 rust::Slice<const double> weights,
                                                 rust::Slice<const double> knots,
                                                 rust::Slice<const int32_t> multiplicities,
                                                 int degree,
                                                 bool periodic,
                                                 bool check_rational)
{
  Handle(Geom_BSplineCurve) curve;
  const int pole_count = static_cast<int>(coords.size() / 3);
  const int knot_count = static_cast<int>(knots.size());
  if (pole_count < 2 || knot_count < 1 || degree < 1 || degree > Geom_BSplineCurve::MaxDegree()
      || static_cast<int>(weights.size()) != pole_count
      || static_cast<int>(multiplicities.size()) != knot_count) {
    return curve;
  }
  try {
    TColgp_Array1OfPnt poles(1, pole_count);
    TColStd_Array1OfReal pole_weights(1, pole_count);
    for (int i = 0; i < pole_count; ++i) {
      poles.SetValue(i + 1, gp_Pnt(coords[3 * i], coords[3 * i + 1], coords[3 * i + 2]));
      pole_weights.SetValue(i + 1, weights[i]);
    }
    TColStd_Array1OfReal knot_values(1, knot_count);
    TColStd_Array1OfInteger knot_mults(1, knot_count);
    for (int i = 0; i < knot_count; ++i) {
      knot_values.SetValue(i + 1, knots[i]);
      knot_mults.SetValue(i + 1, multiplicities[i]);
    }
    curve = new Geom_BSplineCurve(poles,
                                  pole_weights,
                                  knot_values,
                                  knot_mults,
                                  degree,
                                  periodic,
                                  check_rational);
  }
  catch (...) {
    curve.Nullify();
  }
  return curve;
}

// The flattened state of a B-spline that Python's `Part.BSplineCurve` mirrors.
// Every value is read back from OCCT, so a query can never disagree with the
// curve `toShape` would build:
//
//   [degree, maxDegree, nbPoles, nbKnots, periodic, rational,
//    start xyz, end xyz,
//    firstParameter, lastParameter, firstUKnotIndex, lastUKnotIndex, continuity,
//    knotSequenceCount, <knot sequence>,
//    knotsCount, <knots>,
//    multsCount, <mults>,
//    polesCount, <poles xyz>,
//    weightsCount, <weights>]
//
// `continuity` is the `GeomAbs_Shape` value (0 = C0, 6 = CN).
inline void fc_bspline_push_state(rust::Vec<double>& out, Handle(Geom_BSplineCurve) curve)
{
  out.push_back(curve->Degree());
  out.push_back(Geom_BSplineCurve::MaxDegree());
  out.push_back(curve->NbPoles());
  out.push_back(curve->NbKnots());
  out.push_back(curve->IsPeriodic() ? 1.0 : 0.0);
  out.push_back(curve->IsRational() ? 1.0 : 0.0);

  fc_curve_push_point(out, curve->StartPoint());
  fc_curve_push_point(out, curve->EndPoint());
  out.push_back(curve->FirstParameter());
  out.push_back(curve->LastParameter());
  out.push_back(curve->FirstUKnotIndex());
  out.push_back(curve->LastUKnotIndex());
  out.push_back(static_cast<double>(curve->Continuity()));

  // The expanded knot sequence: every knot repeated by its multiplicity (a
  // periodic curve uses the shorter sequence upstream's `KnotSequence` takes).
  Standard_Integer sequence_length = 0;
  if (curve->IsPeriodic()) {
    sequence_length = curve->NbPoles() + 2 * curve->Degree() - curve->Multiplicity(1) + 2;
  }
  else {
    for (Standard_Integer i = 1; i <= curve->NbKnots(); ++i) {
      sequence_length += curve->Multiplicity(i);
    }
  }
  TColStd_Array1OfReal sequence(1, sequence_length);
  curve->KnotSequence(sequence);
  out.push_back(sequence_length);
  for (Standard_Integer i = 1; i <= sequence_length; ++i) {
    out.push_back(sequence.Value(i));
  }

  const TColStd_Array1OfReal& knots = curve->Knots();
  out.push_back(knots.Length());
  for (Standard_Integer i = knots.Lower(); i <= knots.Upper(); ++i) {
    out.push_back(knots.Value(i));
  }

  const TColStd_Array1OfInteger& mults = curve->Multiplicities();
  out.push_back(mults.Length());
  for (Standard_Integer i = mults.Lower(); i <= mults.Upper(); ++i) {
    out.push_back(mults.Value(i));
  }

  const TColgp_Array1OfPnt& poles = curve->Poles();
  out.push_back(poles.Length());
  for (Standard_Integer i = poles.Lower(); i <= poles.Upper(); ++i) {
    fc_curve_push_point(out, poles.Value(i));
  }

  // `Geom_BSplineCurve` leaves its weight array null on a non-rational curve
  // (`Weights()` would then dereference a null handle), so read them through
  // the array-filling overload, which reports 1.0 for that case.
  TColStd_Array1OfReal weight_values(1, curve->NbPoles());
  curve->Weights(weight_values);
  out.push_back(weight_values.Length());
  for (Standard_Integer i = weight_values.Lower(); i <= weight_values.Upper(); ++i) {
    out.push_back(weight_values.Value(i));
  }
}

// The state of the B-spline built from explicit poles/weights/knots/mults, or an
// empty vector when the arguments do not describe a valid curve.
inline rust::Vec<double> fc_bspline_probe(rust::Slice<const double> coords,
                                          rust::Slice<const double> weights,
                                          rust::Slice<const double> knots,
                                          rust::Slice<const int32_t> multiplicities,
                                          int degree,
                                          bool periodic,
                                          bool check_rational)
{
  rust::Vec<double> out;
  try {
    Handle(Geom_BSplineCurve) curve = fc_bspline_curve(
        coords, weights, knots, multiplicities, degree, periodic, check_rational);
    if (curve.IsNull()) {
      return out;
    }
    fc_bspline_push_state(out, curve);
  }
  catch (...) {
    out.clear();
  }
  return out;
}

// The state of the interpolating B-spline through `coords` (`buildFromPoles`
// with `interpolate=True`, and `interpolate` itself). Empty on failure.
inline rust::Vec<double> fc_bspline_probe_interpolated(rust::Slice<const double> coords, bool periodic)
{
  rust::Vec<double> out;
  const int count = static_cast<int>(coords.size() / 3);
  if (count < 2) {
    return out;
  }
  try {
    Handle(TColgp_HArray1OfPnt) points = new TColgp_HArray1OfPnt(1, count);
    for (int i = 0; i < count; ++i) {
      points->SetValue(i + 1, gp_Pnt(coords[3 * i], coords[3 * i + 1], coords[3 * i + 2]));
    }
    GeomAPI_Interpolate interpolate(points, periodic, 1e-7);
    interpolate.Perform();
    if (!interpolate.IsDone()) {
      return out;
    }
    Handle(Geom_BSplineCurve) curve = interpolate.Curve();
    if (curve.IsNull()) {
      return out;
    }
    fc_bspline_push_state(out, curve);
  }
  catch (...) {
    out.clear();
  }
  return out;
}

// Rebuild the curve, convert it to (or from) the periodic form, and return the
// new state. `SetPeriodic`/`SetNotPeriodic` restructure the knots and poles, so
// this cannot be mirrored by editing the flat arrays. Empty on failure.
inline rust::Vec<double> fc_bspline_set_periodic(rust::Slice<const double> coords,
                                                 rust::Slice<const double> weights,
                                                 rust::Slice<const double> knots,
                                                 rust::Slice<const int32_t> multiplicities,
                                                 int degree,
                                                 bool periodic,
                                                 bool check_rational,
                                                 bool target_periodic)
{
  rust::Vec<double> out;
  try {
    Handle(Geom_BSplineCurve) curve = fc_bspline_curve(
        coords, weights, knots, multiplicities, degree, periodic, check_rational);
    if (curve.IsNull()) {
      return out;
    }
    if (target_periodic) {
      curve->SetPeriodic();
    }
    else {
      curve->SetNotPeriodic();
    }
    fc_bspline_push_state(out, curve);
  }
  catch (...) {
    out.clear();
  }
  return out;
}

// `BSplineCurve.segment(u1, u2)`: restrict the curve to `[u1, u2]` and return the
// new state (`Geom_BSplineCurve::Segment`). Empty when the curve cannot be built
// or the segmented curve does not report the requested range.
inline rust::Vec<double> fc_bspline_segment(rust::Slice<const double> coords,
                                            rust::Slice<const double> weights,
                                            rust::Slice<const double> knots,
                                            rust::Slice<const int32_t> multiplicities,
                                            int degree,
                                            bool periodic,
                                            bool check_rational,
                                            double u1,
                                            double u2)
{
  rust::Vec<double> out;
  try {
    Handle(Geom_BSplineCurve) curve = fc_bspline_curve(
        coords, weights, knots, multiplicities, degree, periodic, check_rational);
    if (curve.IsNull()) {
      return out;
    }
    curve->Segment(u1, u2);
    if (std::abs(curve->FirstParameter() - u1) > Precision::Approximation()
        || std::abs(curve->LastParameter() - u2) > Precision::Approximation()) {
      return out;
    }
    fc_bspline_push_state(out, curve);
  }
  catch (...) {
    out.clear();
  }
  return out;
}

// `Geom_BSplineCurve::Resolution`: the parameter tolerance for a 3D tolerance.
// NaN when the curve cannot be built.
inline double fc_bspline_resolution(rust::Slice<const double> coords,
                                    rust::Slice<const double> weights,
                                    rust::Slice<const double> knots,
                                    rust::Slice<const int32_t> multiplicities,
                                    int degree,
                                    bool periodic,
                                    bool check_rational,
                                    double tolerance)
{
  try {
    Handle(Geom_BSplineCurve) curve = fc_bspline_curve(
        coords, weights, knots, multiplicities, degree, periodic, check_rational);
    if (curve.IsNull()) {
      return std::numeric_limits<double>::quiet_NaN();
    }
    Standard_Real u_tolerance = 0.0;
    curve->Resolution(tolerance, u_tolerance);
    return u_tolerance;
  }
  catch (...) {
    return std::numeric_limits<double>::quiet_NaN();
  }
}

// A conic from its frame (`gp_Ax2(P, N, Vx)`), the description the Part curve
// binding keeps: `kind` 2 = circle, 3 = ellipse, `x_axis` the direction of
// parameter 0 (the circle's x axis / the ellipse's major axis).
inline Handle(Geom_Conic) fc_conic_from_frame(int kind,
                                             double ox,
                                             double oy,
                                             double oz,
                                             double nx,
                                             double ny,
                                             double nz,
                                             double xx,
                                             double xy,
                                             double xz,
                                             double major,
                                             double minor)
{
  gp_Ax2 axis(gp_Pnt(ox, oy, oz), gp_Dir(nx, ny, nz), gp_Dir(xx, xy, xz));
  if (kind == 2) {
    return new Geom_Circle(gp_Circ(axis, major));
  }
  return new Geom_Ellipse(gp_Elips(axis, major, minor));
}

// A conic over the parameter range [first, last] as a B-spline, flattened like
// `fc_bspline_probe`.
//
// With `nurbs` this is upstream's `GeomConic::toNurbs`: the trimmed conic is
// converted exactly (a rational B-spline, never sampled) and its knots are
// reparametrised back onto [first, last] when the conversion moved them. Without
// it this is `GeomCurve::toBSpline` (`ShapeConstruct_Curve::ConvertToBSpline`,
// which approximates a conic). Empty on failure.
inline rust::Vec<double> fc_conic_to_spline(int kind,
                                            double ox,
                                            double oy,
                                            double oz,
                                            double nx,
                                            double ny,
                                            double nz,
                                            double xx,
                                            double xy,
                                            double xz,
                                            double major,
                                            double minor,
                                            double first,
                                            double last,
                                            bool nurbs)
{
  rust::Vec<double> out;
  try {
    Handle(Geom_Conic) conic = fc_conic_from_frame(
        kind, ox, oy, oz, nx, ny, nz, xx, xy, xz, major, minor);
    Handle(Geom_BSplineCurve) spline;
    if (nurbs) {
      Handle(Geom_Curve) trimmed = new Geom_TrimmedCurve(conic, first, last);
      spline = GeomConvert::CurveToBSplineCurve(trimmed);
      Standard_Real fnew = spline->FirstParameter();
      Standard_Real lnew = spline->LastParameter();
      Standard_Real utol = 0.0;
      if (!spline->IsPeriodic()) {
        spline->Resolution(Precision::Confusion(), utol);
        if (std::abs(first - fnew) > utol || std::abs(last - lnew) > utol) {
          TColStd_Array1OfReal knots(1, spline->NbKnots());
          spline->Knots(knots);
          BSplCLib::Reparametrize(first, last, knots);
          spline->SetKnots(knots);
        }
      }
    }
    else {
      ShapeConstruct_Curve converter;
      spline = converter.ConvertToBSpline(conic, first, last, Precision::Confusion());
    }
    if (spline.IsNull()) {
      return out;
    }
    fc_bspline_push_state(out, spline);
  }
  catch (...) {
    out.clear();
  }
  return out;
}

// The point at parameter `u` on the B-spline with the given arrays (`value(u)`),
// or an empty vector when the curve cannot be built or evaluated.
inline rust::Vec<double> fc_bspline_point(rust::Slice<const double> coords,
                                          rust::Slice<const double> weights,
                                          rust::Slice<const double> knots,
                                          rust::Slice<const int32_t> multiplicities,
                                          int degree,
                                          bool periodic,
                                          bool check_rational,
                                          double u)
{
  rust::Vec<double> out;
  try {
    Handle(Geom_BSplineCurve) curve = fc_bspline_curve(
        coords, weights, knots, multiplicities, degree, periodic, check_rational);
    if (curve.IsNull()) {
      return out;
    }
    fc_curve_push_point(out, curve->Value(u));
  }
  catch (...) {
    out.clear();
  }
  return out;
}

// A B-spline edge from explicit poles/weights/knots/mults. Returns null when the
// arguments do not describe a valid curve.
inline std::unique_ptr<TopoDS_Shape> fc_bspline_edge(rust::Slice<const double> coords,
                                                    rust::Slice<const double> weights,
                                                    rust::Slice<const double> knots,
                                                    rust::Slice<const int32_t> multiplicities,
                                                    int degree,
                                                    bool periodic,
                                                    bool check_rational)
{
  try {
    Handle(Geom_BSplineCurve) curve = fc_bspline_curve(
        coords, weights, knots, multiplicities, degree, periodic, check_rational);
    if (curve.IsNull()) {
      return nullptr;
    }
    BRepBuilderAPI_MakeEdge make(curve);
    if (!make.IsDone()) {
      return nullptr;
    }
    return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(make.Edge()));
  }
  catch (...) {
    return nullptr;
  }
}

// A comp-solid made of the given solids (`Part.CompSolid`). Returns null on
// failure.
inline std::unique_ptr<TopoDS_Shape> fc_comp_solid(const TopTools_ListOfShape& solids)
{
  try {
    TopoDS_CompSolid compsolid;
    BRep_Builder builder;
    builder.MakeCompSolid(compsolid);
    for (TopTools_ListIteratorOfListOfShape it(solids); it.More(); it.Next()) {
      builder.Add(compsolid, it.Value());
    }
    return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(compsolid));
  }
  catch (...) {
    return nullptr;
  }
}

// All split parts of the arguments (`BRepAlgoAPI_BuilderAlgo`, General Fuse).
// Used to split overlapping planar faces into the regions the unified face maker
// returns. Returns null on failure.
inline std::unique_ptr<TopoDS_Shape> fc_general_fuse(const TopTools_ListOfShape& shapes)
{
  if (shapes.IsEmpty()) {
    return nullptr;
  }
  try {
    BRepAlgoAPI_BuilderAlgo builder;
    builder.SetArguments(shapes);
    builder.SetNonDestructive(Standard_True);
    builder.Build();
    if (!builder.IsDone() || builder.Shape().IsNull()) {
      return nullptr;
    }
    return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(builder.Shape()));
  }
  catch (...) {
    return nullptr;
  }
}

// The plane containing every edge, or false when they do not lie on one.
// `BRepLib_FindSurface` is fed copies because an edge's cached surface can
// mislead it (the same trick as `FaceMakerBuildFace::findPlane` upstream).
inline bool fc_curve_find_plane(const TopTools_ListOfShape& edges, gp_Pln& plane)
{
  if (edges.IsEmpty()) {
    return false;
  }
  try {
    BRep_Builder builder;
    TopoDS_Compound compound;
    builder.MakeCompound(compound);
    for (TopTools_ListIteratorOfListOfShape it(edges); it.More(); it.Next()) {
      builder.Add(compound, BRepBuilderAPI_Copy(it.Value()).Shape());
    }
    BRepLib_FindSurface finder(compound, -1, Standard_True);
    if (!finder.Found() || finder.Surface().IsNull()) {
      return false;
    }
    plane = GeomAdaptor_Surface(finder.Surface()).Plane();
    return true;
  }
  catch (...) {
    return false;
  }
}

// Split each edge at the points where it crosses itself. The graph splitter
// below only sees intersections between different edges, so a figure-eight
// B-spline (one closed edge crossing itself) needs this projection pass first.
// Fragments are appended in order; an edge that cannot be handled is kept whole.
inline void fc_curve_split_self_intersecting(const TopTools_ListOfShape& edges,
                                             const gp_Pln& plane,
                                             TopTools_ListOfShape& out)
{
  const Standard_Real tol = Precision::Confusion();
  for (TopTools_ListIteratorOfListOfShape it(edges); it.More(); it.Next()) {
    const TopoDS_Edge& edge = TopoDS::Edge(it.Value());
    try {
      Standard_Real first = 0.0;
      Standard_Real last = 0.0;
      Handle(Geom_Curve) curve = BRep_Tool::Curve(edge, first, last);
      // Lines and conics never cross themselves, so skip the costly projection.
      if (curve.IsNull() || curve->IsKind(STANDARD_TYPE(Geom_Line))
          || curve->IsKind(STANDARD_TYPE(Geom_Conic))) {
        out.Append(edge);
        continue;
      }
      if (curve->IsKind(STANDARD_TYPE(Geom_TrimmedCurve))) {
        Handle(Geom_TrimmedCurve) trimmed = Handle(Geom_TrimmedCurve)::DownCast(curve);
        if (!trimmed.IsNull()) {
          first = trimmed->FirstParameter();
          last = trimmed->LastParameter();
          curve = trimmed->BasisCurve();
        }
      }
      Handle(Geom2d_Curve) curve2d = GeomAPI::To2d(curve, plane);
      if (curve2d.IsNull()) {
        out.Append(edge);
        continue;
      }
      Geom2dAPI_InterCurveCurve self_intersection(curve2d, tol);
      if (self_intersection.NbPoints() == 0) {
        out.Append(edge);
        continue;
      }
      std::vector<Standard_Real> params;
      for (Standard_Integer i = 1; i <= self_intersection.NbPoints(); ++i) {
        Geom2dAPI_ProjectPointOnCurve projection(self_intersection.Point(i), curve2d, first, last);
        for (Standard_Integer j = 1; j <= projection.NbPoints(); ++j) {
          const Standard_Real param = projection.Parameter(j);
          if (param - first > tol && last - param > tol) {
            params.push_back(param);
          }
        }
      }
      if (params.empty()) {
        out.Append(edge);
        continue;
      }
      std::sort(params.begin(), params.end());
      params.erase(std::unique(params.begin(), params.end(),
                               [tol](double a, double b) { return b - a < tol; }),
                   params.end());

      TopTools_ListOfShape fragments;
      Standard_Real previous = first;
      for (const Standard_Real param : params) {
        if (param - previous > tol) {
          BRepBuilderAPI_MakeEdge make(curve, previous, param);
          if (make.IsDone()) {
            fragments.Append(make.Edge());
          }
          previous = param;
        }
      }
      if (last - previous > tol) {
        BRepBuilderAPI_MakeEdge make(curve, previous, last);
        if (make.IsDone()) {
          fragments.Append(make.Edge());
        }
      }
      if (fragments.IsEmpty()) {
        out.Append(edge);
        continue;
      }
      for (TopTools_ListIteratorOfListOfShape fragment(fragments); fragment.More();
           fragment.Next()) {
        out.Append(fragment.Value());
      }
    }
    catch (...) {
      out.Append(edge);
    }
  }
}

// Split the edges against each other at their intersections. Passing the
// exploded edges (rather than the wire) is what lets General Fuse see where two
// edges cross; on failure the input is returned unchanged.
inline void fc_curve_split_at_intersections(const TopTools_ListOfShape& edges,
                                            TopTools_ListOfShape& out)
{
  if (edges.Extent() <= 1) {
    out = edges;
    return;
  }
  try {
    BRepAlgoAPI_BuilderAlgo splitter;
    splitter.SetArguments(edges);
    splitter.SetNonDestructive(Standard_True);
    splitter.Build();
    if (!splitter.IsDone() || splitter.Shape().IsNull()) {
      out = edges;
      return;
    }
    for (TopExp_Explorer edge(splitter.Shape(), TopAbs_EDGE); edge.More(); edge.Next()) {
      out.Append(edge.Current());
    }
    if (out.IsEmpty()) {
      out = edges;
    }
  }
  catch (...) {
    out = edges;
  }
}

// Build the bounded regions the edges enclose inside `plane`.
// `BOPAlgo_BuilderFace` finds every region regardless of how the loops meet,
// which is what a self-intersecting boundary needs. Returns false when no
// region is bounded (open or non-planar boundaries).
inline bool fc_curve_collect_bounded_faces(const TopTools_ListOfShape& edges,
                                           const gp_Pln& plane,
                                           std::vector<TopoDS_Face>& faces)
{
  if (edges.IsEmpty()) {
    return false;
  }
  try {
    Bnd_Box box;
    for (TopTools_ListIteratorOfListOfShape it(edges); it.More(); it.Next()) {
      BRepBndLib::Add(it.Value(), box);
    }
    // The base face must enclose the geometry so the unbounded exterior is one
    // huge region that can be discarded by its extent below.
    const Standard_Real extent =
        std::max(1.0e8, 10.0 * std::sqrt(std::max(0.0, box.SquareExtent())));
    BRepBuilderAPI_MakeFace make(plane, -extent, extent, -extent, extent);
    if (!make.IsDone() || make.Face().IsNull()) {
      return false;
    }
    TopoDS_Face base = make.Face();
    base.Orientation(TopAbs_FORWARD);

    TopTools_ListOfShape face_edges;
    for (TopTools_ListIteratorOfListOfShape it(edges); it.More(); it.Next()) {
      const TopoDS_Edge& edge = TopoDS::Edge(it.Value());
      face_edges.Append(edge.Oriented(TopAbs_FORWARD));
      face_edges.Append(edge.Oriented(TopAbs_REVERSED));
    }
    BRepLib::BuildPCurveForEdgesOnPlane(face_edges, base);

    BOPAlgo_BuilderFace builder;
    builder.SetFace(base);
    builder.SetShapes(face_edges);
    // Dangling edges would otherwise become internal wires that produce
    // degenerate regions when the face is extruded.
    builder.SetAvoidInternalShapes(Standard_True);
    builder.Perform();
    if (builder.HasErrors()) {
      return false;
    }

    const Standard_Real outer_threshold = extent * extent;
    for (TopTools_ListIteratorOfListOfShape it(builder.Areas()); it.More(); it.Next()) {
      Bnd_Box face_box;
      BRepBndLib::Add(it.Value(), face_box);
      if (face_box.SquareExtent() > outer_threshold) {
        continue;
      }
      GProp_GProps props;
      BRepGProp::SurfaceProperties(it.Value(), props);
      if (props.Mass() < Precision::Confusion()) {
        continue;
      }
      faces.push_back(TopoDS::Face(it.Value()));
    }
    return !faces.empty();
  }
  catch (...) {
    return false;
  }
}

// Split one (possibly self-intersecting) wire at its self-intersections and
// build a planar face for every bounded region. Returns a compound of faces, or
// a null shape when the wire bounds no planar region.
inline std::unique_ptr<TopoDS_Shape> fc_split_wire_into_faces(const TopoDS_Shape& wire)
{
  if (wire.IsNull() || wire.ShapeType() != TopAbs_WIRE) {
    return nullptr;
  }
  try {
    // Work on a private copy: building pcurves below attaches them to edges'
    // shared TShapes, and this helper falls back to the caller's wire when it
    // finds no region. Copying the whole wire keeps the edges connected to each
    // other while isolating them from the caller.
    TopoDS_Shape source = BRepBuilderAPI_Copy(wire).Shape();
    if (source.IsNull()) {
      source = wire;
    }
    TopTools_ListOfShape edges;
    for (TopExp_Explorer edge(source, TopAbs_EDGE); edge.More(); edge.Next()) {
      edges.Append(edge.Current());
    }
    if (edges.IsEmpty()) {
      return nullptr;
    }

    gp_Pln plane;
    if (!fc_curve_find_plane(edges, plane)) {
      return nullptr;
    }

    // A single edge may cross itself (figure-eight B-spline); split that first,
    // then split the fragments against each other (crossing edges, figure-eight
    // polygons).
    TopTools_ListOfShape self_split;
    fc_curve_split_self_intersecting(edges, plane, self_split);
    TopTools_ListOfShape split;
    fc_curve_split_at_intersections(self_split, split);

    std::vector<TopoDS_Face> faces;
    if (!fc_curve_collect_bounded_faces(split, plane, faces)) {
      return nullptr;
    }

    TopoDS_Compound compound;
    BRep_Builder builder;
    builder.MakeCompound(compound);
    for (const TopoDS_Face& face : faces) {
      builder.Add(compound, face);
    }
    return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(compound));
  }
  catch (...) {
    return nullptr;
  }
}

// A filled face through the given boundary wires (`BRepFill_Filling`). Used for
// non-planar boundaries, which `BRepBuilderAPI_MakeFace` cannot build. Returns a
// null shape on any failure, including an OCCT exception — nothing may cross the
// bridge.
inline std::unique_ptr<TopoDS_Shape> fc_fill_wires(const TopTools_ListOfShape& wires)
{
  if (wires.IsEmpty()) {
    return nullptr;
  }
  try {
    BRepFill_Filling filler;
    for (TopTools_ListIteratorOfListOfShape it(wires); it.More(); it.Next()) {
      const TopoDS_Shape& wire = it.Value();
      if (wire.IsNull() || wire.ShapeType() != TopAbs_WIRE) {
        return nullptr;
      }
      for (TopExp_Explorer edge(wire, TopAbs_EDGE); edge.More(); edge.Next()) {
        filler.Add(TopoDS::Edge(edge.Current()), GeomAbs_C0, Standard_True);
      }
    }
    filler.Build();
    if (!filler.IsDone() || filler.Face().IsNull()) {
      return nullptr;
    }
    return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(filler.Face()));
  }
  catch (...) {
    return nullptr;
  }
}

// A ruled surface spanning two edges (`BRepFill::Face`). Both arguments must be
// edges; returns null on any failure, including an OCCT exception.
inline std::unique_ptr<TopoDS_Shape> fc_ruled_surface(const TopoDS_Shape& first,
                                                      const TopoDS_Shape& second)
{
  if (first.IsNull() || second.IsNull() || first.ShapeType() != TopAbs_EDGE
      || second.ShapeType() != TopAbs_EDGE) {
    return nullptr;
  }
  try {
    const TopoDS_Face face = BRepFill::Face(TopoDS::Edge(first), TopoDS::Edge(second));
    if (face.IsNull()) {
      return nullptr;
    }
    return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(face));
  }
  catch (...) {
    return nullptr;
  }
}

// ---------------------------------------------------------------------------
// Shape operations used by `Part.Shape` (offset, thickness, section, slice)
// ---------------------------------------------------------------------------

// Upstream's `JoinType` enum (0 = arc, 1 = tangent, 2 = intersection).
inline GeomAbs_JoinType fc_join_type(int join)
{
  switch (join) {
    case 1:
      return GeomAbs_Tangent;
    case 2:
      return GeomAbs_Intersection;
    default:
      return GeomAbs_Arc;
  }
}

// Planar offset of a face or a wire (`BRepOffsetAPI_MakeOffset`). A face is
// offset through its boundary wires and rebuilt from the offset wires, so the
// result keeps the face the caller offset; a wire returns the offset wire(s) as
// a compound. Returns null on any failure, including an OCCT exception.
inline std::unique_ptr<TopoDS_Shape> fc_offset(const TopoDS_Shape& shape, double distance, int join)
{
  if (shape.IsNull() || !std::isfinite(distance)) {
    return nullptr;
  }
  try {
    const TopAbs_ShapeEnum type = shape.ShapeType();
    if (type != TopAbs_FACE && type != TopAbs_WIRE) {
      return nullptr;
    }
    std::unique_ptr<BRepOffsetAPI_MakeOffset> op;
    if (type == TopAbs_FACE) {
      op.reset(new BRepOffsetAPI_MakeOffset(TopoDS::Face(shape), fc_join_type(join)));
    }
    else {
      op.reset(new BRepOffsetAPI_MakeOffset(TopoDS::Wire(shape), fc_join_type(join)));
    }
    op->Perform(distance, 0.0);
    if (!op->IsDone() || op->Shape().IsNull()) {
      return nullptr;
    }
    if (type != TopAbs_FACE) {
      return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(op->Shape()));
    }

    // The face constructor returns the offset boundary; rebuild the face from
    // those wires. The first one is taken as the outer boundary and the rest as
    // holes, which is right for the single-boundary faces the Part tests use;
    // a multi-wire face is not re-sorted by nesting.
    std::vector<TopoDS_Wire> wires;
    for (TopExp_Explorer it(op->Shape(), TopAbs_WIRE); it.More(); it.Next()) {
      wires.push_back(TopoDS::Wire(it.Current()));
    }
    if (wires.empty()) {
      return nullptr;
    }
    BRepBuilderAPI_MakeFace make(wires.front(), false);
    if (!make.IsDone()) {
      return nullptr;
    }
    for (std::size_t i = 1; i < wires.size(); ++i) {
      make.Add(TopoDS::Wire(wires[i].Reversed()));
    }
    if (!make.IsDone() || make.Face().IsNull()) {
      return nullptr;
    }
    return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(make.Face()));
  }
  catch (...) {
    return nullptr;
  }
}

// Hollow a solid by `offset`, leaving `closing_faces` open
// (`BRepOffsetAPI_MakeThickSolid::MakeThickSolidByJoin`). Returns null on any
// failure, including an OCCT exception.
inline std::unique_ptr<TopoDS_Shape> fc_thick_solid(const TopoDS_Shape& shape,
                                                    const TopTools_ListOfShape& closing_faces,
                                                    double offset,
                                                    double tolerance)
{
  if (shape.IsNull() || !std::isfinite(offset)) {
    return nullptr;
  }
  try {
    BRepOffsetAPI_MakeThickSolid op;
    op.MakeThickSolidByJoin(shape, closing_faces, offset, tolerance, BRepOffset_Skin,
                            Standard_False, Standard_False, GeomAbs_Arc, Standard_False,
                            Message_ProgressRange());
    if (!op.IsDone() || op.Shape().IsNull()) {
      return nullptr;
    }
    return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(op.Shape()));
  }
  catch (...) {
    return nullptr;
  }
}

// The intersection of two shapes (`BRepAlgoAPI_Section`), or null on failure
// (including an OCCT exception).
inline std::unique_ptr<TopoDS_Shape> fc_section(const TopoDS_Shape& first,
                                               const TopoDS_Shape& second)
{
  if (first.IsNull() || second.IsNull()) {
    return nullptr;
  }
  try {
    BRepAlgoAPI_Section op(first, second);
    op.Build();
    if (!op.IsDone() || op.Shape().IsNull()) {
      return nullptr;
    }
    return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(op.Shape()));
  }
  catch (...) {
    return nullptr;
  }
}

// The wires where `shape` meets the plane `dx*x + dy*y + dz*z = distance` —
// upstream `Shape.slice(direction, distance)`. Section returns loose edges, so
// they are chained into wires here; a compound of wires (or null) is returned.
inline std::unique_ptr<TopoDS_Shape> fc_slice(const TopoDS_Shape& shape,
                                             double dx,
                                             double dy,
                                             double dz,
                                             double distance)
{
  if (shape.IsNull() || !std::isfinite(distance)) {
    return nullptr;
  }
  const double magnitude = std::sqrt(dx * dx + dy * dy + dz * dz);
  if (!(magnitude > Precision::Confusion()) || !std::isfinite(magnitude)) {
    return nullptr;
  }
  try {
    // Exactly upstream's plane: `gp_Pln(a, b, c, -d)`.
    const gp_Pln plane(dx, dy, dz, -distance);
    BRepAlgoAPI_Section op(shape, plane);
    op.Build();
    if (!op.IsDone() || op.Shape().IsNull()) {
      return nullptr;
    }

    std::vector<TopoDS_Edge> edges;
    for (TopExp_Explorer it(op.Shape(), TopAbs_EDGE); it.More(); it.Next()) {
      edges.push_back(TopoDS::Edge(it.Current()));
    }
    std::vector<TopoDS_Wire> wires;
    while (!edges.empty()) {
      BRepBuilderAPI_MakeWire make;
      make.Add(edges.front());
      edges.erase(edges.begin());
      bool found = false;
      do {
        found = false;
        for (std::vector<TopoDS_Edge>::iterator it = edges.begin(); it != edges.end(); ++it) {
          make.Add(*it);
          if (make.Error() != BRepBuilderAPI_DisconnectedWire) {
            edges.erase(it);
            found = true;
            break;
          }
        }
      } while (found);
      if (make.IsDone() && !make.Wire().IsNull()) {
        wires.push_back(make.Wire());
      }
    }
    if (wires.empty()) {
      return nullptr;
    }
    TopoDS_Compound compound;
    BRep_Builder builder;
    builder.MakeCompound(compound);
    for (const TopoDS_Wire& wire : wires) {
      builder.Add(compound, wire);
    }
    return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(compound));
  }
  catch (...) {
    return nullptr;
  }
}

// The split parts of input `index` (1-based) from a General Fuse, as a
// compound, or null when that input was not modified or the fuse failed.
inline std::unique_ptr<TopoDS_Shape> fc_general_fuse_modified(
    const TopTools_ListOfShape& shapes,
    int index,
    double tolerance)
{
  if (shapes.IsEmpty() || index < 1) {
    return nullptr;
  }
  try {
    TopoDS_Shape input;
    int position = 1;
    for (TopTools_ListIteratorOfListOfShape it(shapes); it.More(); it.Next(), ++position) {
      if (position == index) {
        input = it.Value();
        break;
      }
    }
    if (input.IsNull()) {
      return nullptr;
    }

    BRepAlgoAPI_BuilderAlgo builder;
    builder.SetArguments(shapes);
    builder.SetNonDestructive(Standard_True);
    if (tolerance > 0.0) {
      builder.SetFuzzyValue(tolerance);
    }
    builder.Build();
    if (!builder.IsDone() || builder.Shape().IsNull()) {
      return nullptr;
    }

    const TopTools_ListOfShape& modified = builder.Modified(input);
    if (modified.IsEmpty()) {
      return nullptr;
    }
    TopoDS_Compound compound;
    BRep_Builder compound_builder;
    compound_builder.MakeCompound(compound);
    for (TopTools_ListIteratorOfListOfShape it(modified); it.More(); it.Next()) {
      compound_builder.Add(compound, it.Value());
    }
    return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(compound));
  }
  catch (...) {
    return nullptr;
  }
}

// `BRepBndLib::AddOptimal` with the protective gap dropped. Fills `out` with
// `[xmin, ymin, zmin, xmax, ymax, zmax]`; false when the shape is null, empty
// or OCCT throws.
inline bool fc_optimal_box(const TopoDS_Shape& shape,
                           bool use_triangulation,
                           bool use_tolerance,
                           rust::Vec<double>& out)
{
  if (shape.IsNull()) {
    return false;
  }
  try {
    Bnd_Box box;
    BRepBndLib::AddOptimal(shape, box, use_triangulation, use_tolerance);
    if (box.IsVoid()) {
      return false;
    }
    box.SetGap(0.0);
    double xmin = 0.0;
    double ymin = 0.0;
    double zmin = 0.0;
    double xmax = 0.0;
    double ymax = 0.0;
    double zmax = 0.0;
    box.Get(xmin, ymin, zmin, xmax, ymax, zmax);
    out.push_back(xmin);
    out.push_back(ymin);
    out.push_back(zmin);
    out.push_back(xmax);
    out.push_back(ymax);
    out.push_back(zmax);
    return true;
  }
  catch (...) {
    return false;
  }
}

// `TopoDS_Shape::IsSame` (same TShape and location, ignoring orientation),
// which `opencascade-sys` does not bridge.
inline bool fc_is_same(const TopoDS_Shape& first, const TopoDS_Shape& second)
{
  try {
    return first.IsSame(second);
  }
  catch (...) {
    return false;
  }
}

// Remove the given sub-shapes (`BRepTools_ReShape`), or null on failure.
inline std::unique_ptr<TopoDS_Shape> fc_reshape_remove(const TopoDS_Shape& shape,
                                                      const TopTools_ListOfShape& removals)
{
  if (shape.IsNull()) {
    return nullptr;
  }
  try {
    BRepTools_ReShape reshape;
    for (TopTools_ListIteratorOfListOfShape it(removals); it.More(); it.Next()) {
      reshape.Remove(it.Value());
    }
    const TopoDS_Shape result = reshape.Apply(shape, TopAbs_SHAPE);
    if (result.IsNull()) {
      return nullptr;
    }
    return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(result));
  }
  catch (...) {
    return nullptr;
  }
}

// Replace the `from` sub-shapes with the matching `to` sub-shapes
// (`BRepTools_ReShape`), or null on failure.
inline std::unique_ptr<TopoDS_Shape> fc_reshape_replace(const TopoDS_Shape& shape,
                                                       const TopTools_ListOfShape& from,
                                                       const TopTools_ListOfShape& to)
{
  if (shape.IsNull() || from.Extent() != to.Extent()) {
    return nullptr;
  }
  try {
    BRepTools_ReShape reshape;
    TopTools_ListIteratorOfListOfShape old_shape(from);
    TopTools_ListIteratorOfListOfShape new_shape(to);
    for (; old_shape.More(); old_shape.Next(), new_shape.Next()) {
      reshape.Replace(old_shape.Value(), new_shape.Value());
    }
    const TopoDS_Shape result = reshape.Apply(shape, TopAbs_SHAPE);
    if (result.IsNull()) {
      return nullptr;
    }
    return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(result));
  }
  catch (...) {
    return nullptr;
  }
}

// `Shape.isValid`: every sub-shape passes `BRepCheck_Analyzer`. False for a
// null shape, or when OCCT throws.
inline bool fc_is_valid(const TopoDS_Shape& shape)
{
  if (shape.IsNull()) {
    return false;
  }
  try {
    BRepCheck_Analyzer analyser(shape, Standard_True);
    return analyser.IsValid() ? true : false;
  }
  catch (...) {
    return false;
  }
}

// `Edge.split`: cut `edge` at the given curve parameters and join the pieces
// into a wire, in parameter order. Mirrors upstream's `TopoShapeEdgePy::split`:
// the pieces are rebuilt on the edge's basis curve and the original edge is not
// touched. Parameters must lie strictly inside the edge's range. Null on
// failure (not an edge, out-of-range parameter, or OCCT throws).
inline std::unique_ptr<TopoDS_Shape> fc_split_edge(const TopoDS_Shape& shape,
                                                   rust::Slice<const double> parameters)
{
  if (shape.IsNull() || shape.ShapeType() != TopAbs_EDGE) {
    return nullptr;
  }
  try {
    const TopoDS_Edge& edge = TopoDS::Edge(shape);
    BRepAdaptor_Curve adapt(edge);
    const double first = adapt.FirstParameter();
    const double last = adapt.LastParameter();

    std::vector<double> par;
    par.reserve(parameters.size() + 2);
    par.push_back(first);
    for (const double value : parameters) {
      if (value <= first || value >= last) {
        return nullptr;
      }
      par.push_back(value);
    }
    par.push_back(last);
    std::sort(par.begin(), par.end());

    Handle(Geom_Curve) curve = adapt.Curve().Curve();
    if (curve.IsNull()) {
      return nullptr;
    }

    BRepBuilderAPI_MakeWire wire;
    BRep_Builder builder;
    for (std::size_t i = 0; i + 1 < par.size(); ++i) {
      BRepBuilderAPI_MakeEdge make(curve, par[i], par[i + 1]);
      if (!make.IsDone()) {
        return nullptr;
      }
      TopoDS_Edge piece = make.Edge();
      // Carry the original edge's pcurves over, so the pieces stay usable in the
      // face the edge belonged to (upstream does the same).
      builder.Transfert(edge, piece);
      builder.Range(piece, par[i], par[i + 1], Standard_False);
      wire.Add(piece);
    }
    if (!wire.IsDone()) {
      return nullptr;
    }
    return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(wire.Shape()));
  }
  catch (...) {
    return nullptr;
  }
}

// `Part.makeSplitShape`: `BRepFeat_SplitShape` over `shape`. `tools` and
// `faces` are parallel lists: each tool (edge/wire) splits the matching face.
// Fills `direct` with `DirectLeft()` and `left` with `Left()`, upstream's two
// result groups. False on failure.
inline bool fc_split_shape(const TopoDS_Shape& shape,
                           const TopTools_ListOfShape& tools,
                           const TopTools_ListOfShape& faces,
                           bool check_interior,
                           TopTools_ListOfShape& direct,
                           TopTools_ListOfShape& left)
{
  if (shape.IsNull() || tools.Extent() != faces.Extent()) {
    return false;
  }
  try {
    BRepFeat_SplitShape split(shape);
    split.SetCheckInterior(check_interior ? Standard_True : Standard_False);

    TopTools_ListIteratorOfListOfShape tool(tools);
    TopTools_ListIteratorOfListOfShape face(faces);
    for (; tool.More(); tool.Next(), face.Next()) {
      const TopoDS_Shape& cutter = tool.Value();
      const TopoDS_Shape& target = face.Value();
      if (cutter.IsNull() || target.IsNull() || target.ShapeType() != TopAbs_FACE) {
        return false;
      }
      if (cutter.ShapeType() == TopAbs_EDGE) {
        split.Add(TopoDS::Edge(cutter), TopoDS::Face(target));
      }
      else if (cutter.ShapeType() == TopAbs_WIRE) {
        split.Add(TopoDS::Wire(cutter), TopoDS::Face(target));
      }
      else {
        return false;
      }
    }

    split.Build();
    direct.Clear();
    left.Clear();
    for (TopTools_ListIteratorOfListOfShape it(split.DirectLeft()); it.More(); it.Next()) {
      direct.Append(it.Value());
    }
    for (TopTools_ListIteratorOfListOfShape it(split.Left()); it.More(); it.Next()) {
      left.Append(it.Value());
    }
    return true;
  }
  catch (...) {
    return false;
  }
}

// `Part.makeSweepSurface`: sweep `profile` along `spine` with
// `BRepOffsetAPI_MakePipe`. The spine may be a wire or a single edge. Returns
// the lateral surface, or null on failure. `tolerance` is accepted for upstream
// signature parity; `BRepOffsetAPI_MakePipe` has no approximation tolerance.
inline std::unique_ptr<TopoDS_Shape> fc_pipe(const TopoDS_Shape& spine,
                                             const TopoDS_Shape& profile,
                                             double tolerance)
{
  if (spine.IsNull() || profile.IsNull()) {
    return nullptr;
  }
  (void)tolerance;
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

    BRepOffsetAPI_MakePipe pipe(wire, profile);
    if (!pipe.IsDone()) {
      return nullptr;
    }
    const TopoDS_Shape result = pipe.Shape();
    if (result.IsNull()) {
      return nullptr;
    }
    return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(result));
  }
  catch (...) {
    return nullptr;
  }
}

// The 3x3 inertia matrix of `shape`, row-major, as nine values. Empty when the
// shape cannot be measured or OCCT throws.
inline void fc_inertia_matrix(const TopoDS_Shape& shape, rust::Vec<double>& out)
{
  if (shape.IsNull()) {
    return;
  }
  try {
    GProp_GProps props;
    BRepGProp::VolumeProperties(shape, props);
    const gp_Mat matrix = props.MatrixOfInertia();
    for (int i = 1; i <= 3; ++i) {
      for (int j = 1; j <= 3; ++j) {
        out.push_back(matrix(i, j));
      }
    }
  }
  catch (...) {
    out.clear();
  }
}

// `Shape.PrincipalProperties`, flattened:
//
//   [symmetry axis, symmetry point,
//    moment1, moment2, moment3,
//    axis1 xyz, axis2 xyz, axis3 xyz,
//    radius1, radius2, radius3]
//
// Empty when the shape cannot be measured or OCCT throws.
inline void fc_principal_properties(const TopoDS_Shape& shape, rust::Vec<double>& out)
{
  if (shape.IsNull()) {
    return;
  }
  try {
    GProp_GProps props;
    BRepGProp::VolumeProperties(shape, props);
    const GProp_PrincipalProps principal = props.PrincipalProperties();

    out.push_back(principal.HasSymmetryAxis() ? 1.0 : 0.0);
    out.push_back(principal.HasSymmetryPoint() ? 1.0 : 0.0);

    double lx = 0.0;
    double ly = 0.0;
    double lz = 0.0;
    principal.Moments(lx, ly, lz);
    out.push_back(lx);
    out.push_back(ly);
    out.push_back(lz);

    fc_curve_push_dir(out, principal.FirstAxisOfInertia());
    fc_curve_push_dir(out, principal.SecondAxisOfInertia());
    fc_curve_push_dir(out, principal.ThirdAxisOfInertia());

    double rxx = 0.0;
    double ryy = 0.0;
    double rzz = 0.0;
    principal.RadiusOfGyration(rxx, ryy, rzz);
    out.push_back(rxx);
    out.push_back(ryy);
    out.push_back(rzz);
  }
  catch (...) {
    out.clear();
  }
}
