#pragma once

// FerroCAD: the general (affine) shape transform `opencascade-sys` 0.3 does not
// bridge.
//
// The crate's `BRepBuilderAPI_Transform` path takes a `gp_Trsf`, which is a
// similarity: it can rotate, translate and scale uniformly, but cannot represent
// a non-uniform scale or shear. `BRepBuilderAPI_GTransform` takes a `gp_GTrsf`
// instead and accepts any affine 3x4 matrix, which is what `App::Link`'s
// `ScaleVector` needs.
//
// The matrix arrives row-major (`m[0..3]` is the first row), matching
// `Matrix4`; OCCT's `gp_GTrsf::SetValue` is 1-based `(row, column)`.
//
// All functions are `inline`, so they are emitted into the cxx-generated shim and
// no separate translation unit is needed.

#include <BRepBuilderAPI_GTransform.hxx>
#include <TopoDS_Shape.hxx>
#include <gp_GTrsf.hxx>

#include <memory>

#include "rust/cxx.h"

inline std::unique_ptr<TopoDS_Shape> fc_gtransform(const TopoDS_Shape& shape,
                                                   rust::Slice<const double> matrix)
{
    if (shape.IsNull() || matrix.size() < 16) {
        return nullptr;
    }
    try {
        gp_GTrsf trsf;
        for (int row = 1; row <= 3; ++row) {
            for (int column = 1; column <= 4; ++column) {
                trsf.SetValue(row, column, matrix[(row - 1) * 4 + (column - 1)]);
            }
        }
        BRepBuilderAPI_GTransform op(shape, trsf, Standard_True);
        if (!op.IsDone()) {
            return nullptr;
        }
        const TopoDS_Shape& result = op.Shape();
        if (result.IsNull()) {
            return nullptr;
        }
        return std::unique_ptr<TopoDS_Shape>(new TopoDS_Shape(result));
    }
    catch (...) {
        return nullptr;
    }
}
