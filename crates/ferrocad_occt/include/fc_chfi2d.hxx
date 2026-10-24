#pragma once

// FerroCAD: the `ChFi2d_*` planar fillet/chamfer algorithms behind
// `Part.ChFi2d`.
//
// `opencascade-sys` 0.3 does not bridge OCCT's ChFi2d package, so this is a
// sibling cxx bridge (like `fc_history.hxx`) over the same global `TopoDS_*`
// types. The algorithms are stored by `unique_ptr` (they are plain C++ classes,
// not `Standard_Transient`), and every method is `const`: it mutates through the
// pointer, so Rust needs no `Pin`.
//
// OCCT's `perform()` throws when the algorithm was never initialised. That is
// reported as the `-1` status code (the Rust layer maps it to `Part.OCCError`,
// upstream's `Base.CADKernelError`); no exception crosses the cxx bridge.

#include <ChFi2d_AnaFilletAlgo.hxx>
#include <ChFi2d_ChamferAPI.hxx>
#include <ChFi2d_FilletAlgo.hxx>
#include <Standard_Failure.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Edge.hxx>
#include <TopoDS_Shape.hxx>
#include <TopoDS_Wire.hxx>
#include <gp_Dir.hxx>
#include <gp_Pln.hxx>
#include <gp_Pnt.hxx>

#include <memory>

#include "rust/cxx.h"

namespace fc_chfi2d_internal
{
inline gp_Pln plane(double ox, double oy, double oz, double nx, double ny, double nz)
{
    return gp_Pln(gp_Pnt(ox, oy, oz), gp_Dir(nx, ny, nz));
}
}  // namespace fc_chfi2d_internal

// The three edges an algorithm returns: the new fillet/chamfer edge followed by
// the two trimmed input edges.
class FcShapeTriple
{
public:
    TopoDS_Shape first;
    TopoDS_Shape second;
    TopoDS_Shape third;

    const TopoDS_Shape& first_shape() const { return first; }
    const TopoDS_Shape& second_shape() const { return second; }
    const TopoDS_Shape& third_shape() const { return third; }

    bool first_is_null() const { return first.IsNull(); }
    bool second_is_null() const { return second.IsNull(); }
    bool third_is_null() const { return third.IsNull(); }
};

// --- ChFi2d_FilletAlgo -----------------------------------------------------

class FcChFi2dFilletAlgo
{
public:
    std::unique_ptr<ChFi2d_FilletAlgo> algo;

    FcChFi2dFilletAlgo() : algo(std::make_unique<ChFi2d_FilletAlgo>()) {}

    void init_wire(const TopoDS_Shape& wire, double ox, double oy, double oz, double nx,
                   double ny, double nz) const
    {
        try {
            algo->Init(TopoDS::Wire(wire),
                       fc_chfi2d_internal::plane(ox, oy, oz, nx, ny, nz));
        }
        catch (...) {
        }
    }

    void init_edges(const TopoDS_Shape& edge1, const TopoDS_Shape& edge2, double ox,
                    double oy, double oz, double nx, double ny, double nz) const
    {
        try {
            algo->Init(TopoDS::Edge(edge1), TopoDS::Edge(edge2),
                       fc_chfi2d_internal::plane(ox, oy, oz, nx, ny, nz));
        }
        catch (...) {
        }
    }

    // 1 = performed, 0 = no result, -1 = OCCT raised (e.g. never initialised).
    int perform(double radius) const
    {
        try {
            return algo->Perform(radius) ? 1 : 0;
        }
        catch (const Standard_Failure&) {
            return -1;
        }
        catch (...) {
            return -1;
        }
    }

    // The number of solutions, or -1 when OCCT raised.
    int nb_results(double x, double y, double z) const
    {
        try {
            return algo->NbResults(gp_Pnt(x, y, z));
        }
        catch (...) {
            return -1;
        }
    }

    std::unique_ptr<FcShapeTriple> result(double x, double y, double z,
                                          int solution) const
    {
        try {
            TopoDS_Edge edge1;
            TopoDS_Edge edge2;
            TopoDS_Edge fillet = algo->Result(gp_Pnt(x, y, z), edge1, edge2, solution);
            auto out = std::make_unique<FcShapeTriple>();
            out->first = fillet;
            out->second = edge1;
            out->third = edge2;
            return out;
        }
        catch (...) {
            return nullptr;
        }
    }
};

inline std::unique_ptr<FcChFi2dFilletAlgo> fc_chfi2d_fillet_algo_new()
{
    return std::make_unique<FcChFi2dFilletAlgo>();
}

// --- ChFi2d_AnaFilletAlgo --------------------------------------------------

class FcChFi2dAnaFilletAlgo
{
public:
    std::unique_ptr<ChFi2d_AnaFilletAlgo> algo;

    FcChFi2dAnaFilletAlgo() : algo(std::make_unique<ChFi2d_AnaFilletAlgo>()) {}

    void init_wire(const TopoDS_Shape& wire, double ox, double oy, double oz, double nx,
                   double ny, double nz) const
    {
        try {
            algo->Init(TopoDS::Wire(wire),
                       fc_chfi2d_internal::plane(ox, oy, oz, nx, ny, nz));
        }
        catch (...) {
        }
    }

    void init_edges(const TopoDS_Shape& edge1, const TopoDS_Shape& edge2, double ox,
                    double oy, double oz, double nx, double ny, double nz) const
    {
        try {
            algo->Init(TopoDS::Edge(edge1), TopoDS::Edge(edge2),
                       fc_chfi2d_internal::plane(ox, oy, oz, nx, ny, nz));
        }
        catch (...) {
        }
    }

    bool perform(double radius) const
    {
        try {
            return algo->Perform(radius);
        }
        catch (...) {
            return false;
        }
    }

    std::unique_ptr<FcShapeTriple> result() const
    {
        try {
            TopoDS_Edge edge1;
            TopoDS_Edge edge2;
            TopoDS_Shape fillet = algo->Result(edge1, edge2);
            auto out = std::make_unique<FcShapeTriple>();
            out->first = fillet;
            out->second = edge1;
            out->third = edge2;
            return out;
        }
        catch (...) {
            return nullptr;
        }
    }
};

inline std::unique_ptr<FcChFi2dAnaFilletAlgo> fc_chfi2d_ana_fillet_algo_new()
{
    return std::make_unique<FcChFi2dAnaFilletAlgo>();
}

// --- ChFi2d_ChamferAPI -----------------------------------------------------

class FcChFi2dChamferAPI
{
public:
    std::unique_ptr<ChFi2d_ChamferAPI> algo;

    FcChFi2dChamferAPI() : algo(std::make_unique<ChFi2d_ChamferAPI>()) {}

    void init_wire(const TopoDS_Shape& wire) const
    {
        try {
            algo->Init(TopoDS::Wire(wire));
        }
        catch (...) {
        }
    }

    void init_edges(const TopoDS_Shape& edge1, const TopoDS_Shape& edge2) const
    {
        try {
            algo->Init(TopoDS::Edge(edge1), TopoDS::Edge(edge2));
        }
        catch (...) {
        }
    }

    bool perform() const
    {
        try {
            return algo->Perform();
        }
        catch (...) {
            return false;
        }
    }

    std::unique_ptr<FcShapeTriple> result(double length1, double length2) const
    {
        try {
            TopoDS_Edge edge1;
            TopoDS_Edge edge2;
            TopoDS_Edge chamfer = algo->Result(edge1, edge2, length1, length2);
            auto out = std::make_unique<FcShapeTriple>();
            out->first = chamfer;
            out->second = edge1;
            out->third = edge2;
            return out;
        }
        catch (...) {
            return nullptr;
        }
    }
};

inline std::unique_ptr<FcChFi2dChamferAPI> fc_chfi2d_chamfer_api_new()
{
    return std::make_unique<FcChFi2dChamferAPI>();
}
