#pragma once

// FerroCAD: the analytic curve behind an edge (`edge.Curve` upstream).
//
// `opencascade-sys` 0.3 bridges `BRep_Tool::Curve` but not the curve's dynamic
// type or a `Geom_*`'s parameters, so the classification lives here. Companion to
// `fc_history.hxx` / `fc_topology.hxx`; see `docs/tasks/p2-architecture.md` P2.2.
//
// The result is one flat `rust::Vec<double>` (a shared struct would have to be
// declared in the cxx bridge both ways):
//
//   [kind, ox, oy, oz, dx, dy, dz, xx, xy, xz, first, last, major, minor]
//
// kind: 0 = unsupported, 1 = line, 2 = circle/arc, 3 = ellipse.
// `o` is the line origin / circle centre / ellipse centre; `d` the line direction
// or the plane normal; `x` the circle/ellipse's x axis (major axis for an
// ellipse); `first`/`last` the trimmed parameter range; `major` the circle radius
// or the ellipse major radius; `minor` the ellipse minor radius (unused for a
// circle). On kind 0 every other value is 0.

#include <BRep_Tool.hxx>
#include <Geom_Circle.hxx>
#include <Geom_Curve.hxx>
#include <Geom_Ellipse.hxx>
#include <Geom_Line.hxx>
#include <Geom_TrimmedCurve.hxx>
#include <Standard_Type.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Edge.hxx>
#include <TopoDS_Shape.hxx>
#include <TopoDS_Vertex.hxx>
#include <TopoDS_Wire.hxx>
#include <gp_Ax1.hxx>
#include <gp_Ax2.hxx>
#include <gp_Circ.hxx>
#include <gp_Dir.hxx>
#include <gp_Elips.hxx>
#include <gp_Lin.hxx>
#include <gp_Pnt.hxx>

#include "rust/cxx.h"

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

inline rust::Vec<double> fc_edge_curve(const TopoDS_Shape& shape)
{
  rust::Vec<double> out;
  out.reserve(14);
  out.push_back(0.0); // kind, replaced below
  for (int i = 0; i < 13; ++i) {
    out.push_back(0.0);
  }

  if (shape.IsNull() || shape.ShapeType() != TopAbs_EDGE) {
    return out;
  }
  const TopoDS_Edge& edge = TopoDS::Edge(shape);
  double first = 0.0;
  double last = 0.0;
  Handle(Geom_Curve) curve = BRep_Tool::Curve(edge, first, last);
  if (curve.IsNull()) {
    return out;
  }

  // `BRep_Tool::Curve` keeps the trimming curve for an arc; classify its basis.
  if (curve->IsKind(STANDARD_TYPE(Geom_TrimmedCurve))) {
    Handle(Geom_TrimmedCurve) trimmed = Handle(Geom_TrimmedCurve)::DownCast(curve);
    if (!trimmed.IsNull()) {
      first = trimmed->FirstParameter();
      last = trimmed->LastParameter();
      curve = trimmed->BasisCurve();
    }
  }

  rust::Vec<double> data;
  data.reserve(14);
  data.push_back(0.0);
  const gp_Pnt origin(0.0, 0.0, 0.0);
  const gp_Dir direction(0.0, 0.0, 1.0);
  fc_curve_push_point(data, origin);
  fc_curve_push_dir(data, direction);
  fc_curve_push_dir(data, direction);
  data.push_back(first);
  data.push_back(last);
  data.push_back(0.0);
  data.push_back(0.0);

  if (curve->IsKind(STANDARD_TYPE(Geom_Line))) {
    Handle(Geom_Line) line = Handle(Geom_Line)::DownCast(curve);
    if (line.IsNull()) {
      return out;
    }
    const gp_Lin& lin = line->Lin();
    data[0] = 1.0;
    data[1] = lin.Location().X();
    data[2] = lin.Location().Y();
    data[3] = lin.Location().Z();
    data[4] = lin.Direction().X();
    data[5] = lin.Direction().Y();
    data[6] = lin.Direction().Z();
    return data;
  }

  if (curve->IsKind(STANDARD_TYPE(Geom_Circle))) {
    Handle(Geom_Circle) circle = Handle(Geom_Circle)::DownCast(curve);
    if (circle.IsNull()) {
      return out;
    }
    const gp_Circ& circ = circle->Circ();
    const gp_Ax2& position = circ.Position();
    data[0] = 2.0;
    data[1] = position.Location().X();
    data[2] = position.Location().Y();
    data[3] = position.Location().Z();
    data[4] = position.Direction().X();
    data[5] = position.Direction().Y();
    data[6] = position.Direction().Z();
    data[7] = position.XDirection().X();
    data[8] = position.XDirection().Y();
    data[9] = position.XDirection().Z();
    data[12] = circ.Radius();
    return data;
  }

  if (curve->IsKind(STANDARD_TYPE(Geom_Ellipse))) {
    Handle(Geom_Ellipse) ellipse = Handle(Geom_Ellipse)::DownCast(curve);
    if (ellipse.IsNull()) {
      return out;
    }
    const gp_Elips& elips = ellipse->Elips();
    const gp_Ax2& position = elips.Position();
    data[0] = 3.0;
    data[1] = position.Location().X();
    data[2] = position.Location().Y();
    data[3] = position.Location().Z();
    data[4] = position.Direction().X();
    data[5] = position.Direction().Y();
    data[6] = position.Direction().Z();
    data[7] = position.XDirection().X();
    data[8] = position.XDirection().Y();
    data[9] = position.XDirection().Z();
    data[12] = elips.MajorRadius();
    data[13] = elips.MinorRadius();
    return data;
  }

  return out;
}

// The edge view of a shape, for `BRepBuilderAPI_MakeWire::Add`.
inline const TopoDS_Edge& fc_shape_as_edge(const TopoDS_Shape& shape)
{
  return TopoDS::Edge(shape);
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
