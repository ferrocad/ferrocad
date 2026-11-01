//! FreeCAD's `TopoShape` operation policy, on top of the generic OCCT bindings.
//!
//! These rules come from FreeCAD, not from OCCT: `Mod/Part/App/TopoShape.cpp`
//! for `isClosed` and the shape tolerance, `FCBRepAlgoAPI_BooleanOperation` for
//! how a boolean operation is configured. They used to be C++ helpers
//! (`include/fc_policy.hxx`) only because the OCCT entry points they need were
//! not bridged; now they are plain Rust over `ferrocad_part_cxx`.

use std::pin::Pin;

use cxx::UniquePtr;
use ferrocad_curves::Vector3;
use ferrocad_geom::SurfaceSpec;
use opencascade_sys as ffi;

use ffi::b_rep_algo_api::BRepAlgoAPI_Cut;
use ffi::b_rep_class::TopAbs_State;
use ffi::geom_abs::GeomAbs_JoinType;
use ffi::top_abs::{TopAbs_Orientation, TopAbs_ShapeEnum};
use ffi::top_tools::{TopTools_ListOfShape};
use ffi::topo_ds::{
    TopoDS, TopoDS_Edge, TopoDS_Face, TopoDS_Shape, TopoDS_Vertex, TopoDS_Wire,
};

use crate::error::OcctError;
use crate::shape::explore;

/// Upstream `FCBRepAlgoAPI_BooleanOperation`'s constructor settings: run in
/// parallel and non-destructively, so the operands are not rewritten in place.
pub(crate) fn cut_prepare(mut op: Pin<&mut BRepAlgoAPI_Cut>) {
    ffi::b_rep_algo_api::BRepAlgoAPI_Cut_SetRunParallel(op.as_mut(), true);
    op.as_mut().SetNonDestructive(true);
}

/// Upstream's automatic fuzzy value: the diagonal of the combined operand bounds
/// times `Precision::Confusion()` (`FuzzyHelper::getBooleanFuzzy` defaults its
/// factor to 1.0).
///
/// Reads the operands already installed on `op`, so it must run after
/// `configure_cut`.
pub(crate) fn cut_set_auto_fuzzy(mut op: Pin<&mut BRepAlgoAPI_Cut>) {
    let mut bounds = ffi::bnd::Bnd_Box_new();
    add_bounds(op.as_ref().get_ref().Arguments(), bounds.pin_mut());
    add_bounds(op.as_ref().get_ref().Tools(), bounds.pin_mut());

    let diagonal = bounds.as_ref().unwrap().SquareExtent().sqrt();
    ffi::b_rep_algo_api::BRepAlgoAPI_Cut_SetFuzzyValue(
        op.as_mut(),
        diagonal * ffi::precision::Precision::Confusion(),
    );
}

/// Extend `bounds` with every shape in `shapes`, using the triangulation when the
/// shape has one (`BRepBndLib::Add`'s default, which upstream also relies on).
fn add_bounds(shapes: &TopTools_ListOfShape, mut bounds: Pin<&mut ffi::bnd::Bnd_Box>) {
    let vec = ffi::topo_ds::shape_list_to_vector(shapes);
    for shape in vec.iter() {
        ffi::b_rep_bnd_lib::BRepBndLib::Add(shape, bounds.as_mut(), true);
    }
}

/// Upstream `TopoShape::isClosed`: a shell, wire, edge, face or vertex asks
/// `BRep_Tool::IsClosed`; a solid or comp-solid requires every shell to be
/// closed; a compound also requires every face, wire, edge and vertex that is not
/// contained in a higher element to be closed.
pub(crate) fn is_closed(shape: &TopoDS_Shape) -> bool {
    if shape.IsNull() {
        return false;
    }
    match shape.ShapeType() {
        TopAbs_ShapeEnum::TopAbs_SHELL
        | TopAbs_ShapeEnum::TopAbs_WIRE
        | TopAbs_ShapeEnum::TopAbs_EDGE
        | TopAbs_ShapeEnum::TopAbs_FACE
        | TopAbs_ShapeEnum::TopAbs_VERTEX
        | TopAbs_ShapeEnum::TopAbs_SHAPE => ffi::b_rep::BRep_Tool_IsClosed_shape(shape),
        TopAbs_ShapeEnum::TopAbs_COMPSOLID | TopAbs_ShapeEnum::TopAbs_SOLID => {
            every_sub_shape_closed(
                shape,
                TopAbs_ShapeEnum::TopAbs_SHELL,
                TopAbs_ShapeEnum::TopAbs_SHAPE,
            )
        }
        TopAbs_ShapeEnum::TopAbs_COMPOUND => {
            // Each of these asks only about sub-shapes that are not contained in
            // a higher element, which is what `to_avoid` selects.
            let mut closed = every_sub_shape_closed(
                shape,
                TopAbs_ShapeEnum::TopAbs_SHELL,
                TopAbs_ShapeEnum::TopAbs_SHAPE,
            );
            closed &= every_sub_shape_closed(
                shape,
                TopAbs_ShapeEnum::TopAbs_FACE,
                TopAbs_ShapeEnum::TopAbs_SHELL,
            );
            closed &= every_sub_shape_closed(
                shape,
                TopAbs_ShapeEnum::TopAbs_WIRE,
                TopAbs_ShapeEnum::TopAbs_FACE,
            );
            closed &= every_sub_shape_closed(
                shape,
                TopAbs_ShapeEnum::TopAbs_EDGE,
                TopAbs_ShapeEnum::TopAbs_WIRE,
            );
            closed &= every_sub_shape_closed(
                shape,
                TopAbs_ShapeEnum::TopAbs_VERTEX,
                TopAbs_ShapeEnum::TopAbs_EDGE,
            );
            closed
        }
        // `TopAbs_ShapeEnum` is an extern enum, so the compiler must allow an
        // unknown discriminant; a kind OCCT does not define is not closed.
        _ => false,
    }
}

/// Every sub-shape of type `to_find` that is not inside a `to_avoid` sub-shape
/// reports closed.
fn every_sub_shape_closed(
    shape: &TopoDS_Shape,
    to_find: TopAbs_ShapeEnum,
    to_avoid: TopAbs_ShapeEnum,
) -> bool {
    let found = explore(shape, to_find, to_avoid);
    (1..=found.Extent()).all(|index| ffi::b_rep::BRep_Tool_IsClosed_shape(found.FindKey(index)))
}

/// The shape's geometric tolerance. A vertex, edge or face reports its own; any
/// higher shape reports the maximum over its sub-shapes, matching
/// `ShapeAnalysis_ShapeTolerance`'s default.
pub(crate) fn shape_tolerance(shape: &TopoDS_Shape) -> f64 {
    if shape.IsNull() {
        return 0.0;
    }
    match shape.ShapeType() {
        TopAbs_ShapeEnum::TopAbs_VERTEX
        | TopAbs_ShapeEnum::TopAbs_EDGE
        | TopAbs_ShapeEnum::TopAbs_FACE => leaf_tolerance(shape),
        _ => max_tolerance(shape, TopAbs_ShapeEnum::TopAbs_VERTEX)
            .max(max_tolerance(shape, TopAbs_ShapeEnum::TopAbs_EDGE))
            .max(max_tolerance(shape, TopAbs_ShapeEnum::TopAbs_FACE)),
    }
}

/// The largest tolerance among `shape`'s sub-shapes of `kind`, or 0.0 when it has
/// none.
fn max_tolerance(shape: &TopoDS_Shape, kind: TopAbs_ShapeEnum) -> f64 {
    let found = explore(shape, kind, TopAbs_ShapeEnum::TopAbs_SHAPE);
    (1..=found.Extent())
        .map(|index| leaf_tolerance(found.FindKey(index)))
        .fold(0.0_f64, f64::max)
}

/// `BRep_Tool::Tolerance` of a vertex, edge or face. Those are the only shapes
/// that carry one, so anything else reports 0.0 - which is also what upstream
/// accumulates before walking the sub-shapes.
fn leaf_tolerance(shape: &TopoDS_Shape) -> f64 {
    match shape.ShapeType() {
        TopAbs_ShapeEnum::TopAbs_VERTEX => ffi::b_rep::BRep_Tool_Tolerance_vertex(TopoDS::Vertex(shape)),
        TopAbs_ShapeEnum::TopAbs_EDGE => ffi::b_rep::BRep_Tool_Tolerance_edge(TopoDS::Edge(shape)),
        TopAbs_ShapeEnum::TopAbs_FACE => ffi::b_rep::BRep_Tool_Tolerance_face(TopoDS::Face(shape)),
        _ => 0.0,
    }
}

// ---------------------------------------------------------------------------
// `TopoShape` queries (`shape_ops.hxx`)
// ---------------------------------------------------------------------------

/// `Shape.isInside(point, tolerance, checkFace)`: the lower-dimensional shapes
/// answer with the point-to-shape distance, everything else goes through the
/// solid classifier. Both branches are OCCT calls; the split is FreeCAD's.
pub(crate) fn is_inside(
    shape: &TopoDS_Shape,
    point: Vector3,
    tolerance: f64,
    check_face: bool,
) -> Result<bool, OcctError> {
    if shape.IsNull() || !tolerance.is_finite() {
        return Err(OcctError::NotDone {
            operation: "is_inside",
        });
    }
    let kind = shape.ShapeType();
    let pnt = ffi::gp::new_point(point.x, point.y, point.z);
    if matches!(
        kind,
        TopAbs_ShapeEnum::TopAbs_VERTEX
            | TopAbs_ShapeEnum::TopAbs_EDGE
            | TopAbs_ShapeEnum::TopAbs_WIRE
            | TopAbs_ShapeEnum::TopAbs_FACE
    ) {
        let mut maker = ffi::b_rep_builder_api::BRepBuilderAPI_MakeVertex_gp_Pnt(&pnt);
        let distance = ffi::b_rep_extrema::BRepExtrema_DistShapeShape_new(
            ffi::topo_ds::cast_vertex_to_shape(maker.pin_mut().Vertex()),
            shape,
        );
        if !distance.IsDone() {
            return Err(OcctError::NotDone {
                operation: "is_inside",
            });
        }
        return Ok(distance.Value() <= tolerance);
    }

    let state = ffi::b_rep_class::BRepClass3d_SolidClassifier_Perform(
        shape,
        point.x,
        point.y,
        point.z,
        tolerance,
    )
    .map_err(|e| OcctError::Exception {
        operation: "is_inside",
        message: e.what().to_string(),
    })?;
    let mut inside = state.State() == TopAbs_State::TopAbs_IN;
    if check_face && state.IsOnAFace() {
        inside = true;
    }
    Ok(inside)
}

/// `Shape.complement()`: a copy with the complement of the orientation.
/// `shape_complement` already copies (FerroCAD policy: the shared `TShape` must
/// not be mutated), so this is just the null guard and the result check.
pub(crate) fn complement(shape: &TopoDS_Shape) -> Result<UniquePtr<TopoDS_Shape>, OcctError> {
    if shape.IsNull() {
        return Err(OcctError::NotDone {
            operation: "complement",
        });
    }
    let result = ffi::topo_ds::shape_complement(shape);
    if result.is_null() {
        return Err(OcctError::NotDone {
            operation: "complement",
        });
    }
    Ok(result)
}

/// `Shape.transformGeometry(matrix, copy)`: copy first, then apply the affine
/// `gp_GTrsf` with `BRepBuilderAPI_GTransform`.
pub(crate) fn transform_geometry(
    shape: &TopoDS_Shape,
    matrix: &[f64],
    copy: bool,
) -> Result<UniquePtr<TopoDS_Shape>, OcctError> {
    if shape.IsNull() || matrix.len() < 16 {
        return Err(OcctError::NotDone {
            operation: "transform_geometry",
        });
    }
    let mut trsf = ffi::gp::new_gp_GTrsf();
    for row in 1..=3 {
        for column in 1..=4 {
            trsf.pin_mut()
                .SetValue(row, column, matrix[((row - 1) * 4 + (column - 1)) as usize]);
        }
    }
    let mut copier = ffi::b_rep_builder_api::BRepBuilderAPI_Copy_new(shape, true, false);
    let mut op =
        ffi::b_rep_builder_api::BRepBuilderAPI_GTransform_new(copier.pin_mut().Shape(), &trsf, copy);
    if !op.IsDone() {
        return Err(OcctError::NotDone {
            operation: "transform_geometry",
        });
    }
    let result = ffi::topo_ds::TopoDS_Shape_to_owned(op.pin_mut().Shape());
    if result.IsNull() {
        return Err(OcctError::NotDone {
            operation: "transform_geometry",
        });
    }
    Ok(result)
}

/// `Shape.globalTolerance(mode)`: the average (0), maximum (>0) or minimum (<0)
/// tolerance via the kernel accumulator.
pub(crate) fn global_tolerance(shape: &TopoDS_Shape, mode: i32) -> Result<f64, OcctError> {
    if shape.IsNull() {
        return Err(OcctError::NotDone {
            operation: "global_tolerance",
        });
    }
    let mut analysis = ffi::shape_analysis::ShapeAnalysis_ShapeTolerance_new();
    ffi::shape_analysis::ShapeAnalysis_ShapeTolerance_Tolerance(
        analysis.pin_mut(),
        shape,
        mode,
        TopAbs_ShapeEnum::TopAbs_SHAPE,
    );
    let value = ffi::shape_analysis::ShapeAnalysis_ShapeTolerance_GlobalTolerance(&analysis, mode);
    if !value.is_finite() {
        return Err(OcctError::NotDone {
            operation: "global_tolerance",
        });
    }
    Ok(value)
}

/// `Shape.fixTolerance(value, kind)`: force the selected sub-shape tolerances.
pub(crate) fn fix_tolerance(
    shape: &TopoDS_Shape,
    value: f64,
    kind: TopAbs_ShapeEnum,
) -> Result<(), OcctError> {
    if shape.IsNull() || !value.is_finite() || value < 0.0 {
        return Err(OcctError::InvalidInput {
            what: "a tolerance is a finite, non-negative number",
        });
    }
    let fix = ffi::shape_fix::ShapeFix_ShapeTolerance_new();
    ffi::shape_fix::ShapeFix_ShapeTolerance_SetTolerance(&fix, shape, value, kind).map_err(|e| {
        OcctError::Exception {
            operation: "fix_tolerance",
            message: e.what().to_string(),
        }
    })
}

/// `Shape.limitTolerance(minimum, maximum, kind)`: whether any tolerance changed.
pub(crate) fn limit_tolerance(
    shape: &TopoDS_Shape,
    minimum: f64,
    maximum: f64,
    kind: TopAbs_ShapeEnum,
) -> Result<bool, OcctError> {
    if shape.IsNull() || !minimum.is_finite() || !maximum.is_finite() {
        return Err(OcctError::InvalidInput {
            what: "tolerances are finite numbers",
        });
    }
    let fix = ffi::shape_fix::ShapeFix_ShapeTolerance_new();
    ffi::shape_fix::ShapeFix_ShapeTolerance_LimitTolerance(&fix, shape, minimum, maximum, kind)
        .map_err(|e| OcctError::Exception {
            operation: "limit_tolerance",
            message: e.what().to_string(),
        })
}

/// `Shape.inTolerance(minimum, maximum, kind)`: the matching sub-shapes as a list.
pub(crate) fn tolerance_in(
    shape: &TopoDS_Shape,
    minimum: f64,
    maximum: f64,
    kind: TopAbs_ShapeEnum,
) -> Result<UniquePtr<TopTools_ListOfShape>, OcctError> {
    if shape.IsNull() || !minimum.is_finite() || !maximum.is_finite() {
        return Err(OcctError::InvalidInput {
            what: "tolerances are finite numbers",
        });
    }
    let analysis = ffi::shape_analysis::ShapeAnalysis_ShapeTolerance_new();
    let found =
        ffi::shape_analysis::ShapeAnalysis_ShapeTolerance_InTolerance(&analysis, shape, minimum, maximum, kind);
    let mut list = ffi::top_tools::new_list_of_shape();
    if !found.IsNull() {
        for index in 1..=ffi::top_tools::TopTools_HSequenceOfShape_length(&found) {
            list.pin_mut()
                .Append(ffi::top_tools::TopTools_HSequenceOfShape_value(&found, index));
        }
    }
    Ok(list)
}

/// `Shape.overTolerance(value, kind)`: the matching sub-shapes as a list.
pub(crate) fn tolerance_over(
    shape: &TopoDS_Shape,
    value: f64,
    kind: TopAbs_ShapeEnum,
) -> Result<UniquePtr<TopTools_ListOfShape>, OcctError> {
    if shape.IsNull() || !value.is_finite() {
        return Err(OcctError::InvalidInput {
            what: "the tolerance is a finite number",
        });
    }
    let analysis = ffi::shape_analysis::ShapeAnalysis_ShapeTolerance_new();
    let found =
        ffi::shape_analysis::ShapeAnalysis_ShapeTolerance_OverTolerance(&analysis, shape, value, kind);
    let mut list = ffi::top_tools::new_list_of_shape();
    if !found.IsNull() {
        for index in 1..=ffi::top_tools::TopTools_HSequenceOfShape_length(&found) {
            list.pin_mut()
                .Append(ffi::top_tools::TopTools_HSequenceOfShape_value(&found, index));
        }
    }
    Ok(list)
}

/// `Shape.makeWires()`: sort the edges into wires, repair each with
/// `ShapeFix_Wire`, and return a compound of all of them.
pub(crate) fn make_wires(
    shape: &TopoDS_Shape,
    tolerance: f64,
) -> Result<UniquePtr<TopoDS_Shape>, OcctError> {
    if shape.IsNull() {
        return Err(OcctError::NotDone {
            operation: "make_wires",
        });
    }
    let precision = if tolerance.is_finite() && tolerance >= ffi::precision::Precision::Confusion() {
        tolerance
    } else {
        ffi::precision::Precision::Confusion()
    };

    let mut edges = ffi::top_tools::new_indexed_map_of_shape();
    ffi::top_exp::TopExp::MapShapes(shape, TopAbs_ShapeEnum::TopAbs_EDGE, edges.pin_mut());
    let mut pending: Vec<UniquePtr<TopoDS_Edge>> = (1..=edges.Extent())
        .map(|index| ffi::topo_ds::TopoDS_Edge_to_owned(TopoDS::Edge(edges.FindKey(index))))
        .collect();

    let mut wires: Vec<UniquePtr<TopoDS_Wire>> = Vec::new();
    while !pending.is_empty() {
        let mut maker = ffi::b_rep_builder_api::BRepBuilderAPI_MakeWire_new();
        maker.pin_mut().add_edge(&pending[0]);
        pending.remove(0);
        if !maker.IsDone() {
            continue;
        }
        let mut wire = ffi::topo_ds::TopoDS_Wire_to_owned(maker.pin_mut().Wire());
        loop {
            let mut removed = None;
            for (index, edge) in pending.iter().enumerate() {
                maker.pin_mut().add_edge(edge);
                if maker.Error()
                    != ffi::b_rep_builder_api::BRepBuilderAPI_WireError::BRepBuilderAPI_DisconnectedWire
                {
                    removed = Some(index);
                    wire = ffi::topo_ds::TopoDS_Wire_to_owned(maker.pin_mut().Wire());
                    break;
                }
            }
            match removed {
                Some(index) => {
                    pending.remove(index);
                }
                None => break,
            }
        }

        let fix = ffi::shape_fix::ShapeFix_Wire_new();
        ffi::shape_fix::ShapeFix_Root_set_precision(
            ffi::shape_fix::ShapeFix_Wire_as_root(&fix),
            precision,
        );
        ffi::shape_fix::ShapeFix_Wire_Load(&fix, &wire).map_err(|e| OcctError::Exception {
            operation: "make_wires",
            message: e.what().to_string(),
        })?;
        ffi::shape_fix::ShapeFix_Wire_FixReorder(&fix).map_err(|e| OcctError::Exception {
            operation: "make_wires",
            message: e.what().to_string(),
        })?;
        ffi::shape_fix::ShapeFix_Wire_FixConnected(&fix, -1.0).map_err(|e| OcctError::Exception {
            operation: "make_wires",
            message: e.what().to_string(),
        })?;
        ffi::shape_fix::ShapeFix_Wire_FixClosed(&fix, -1.0).map_err(|e| OcctError::Exception {
            operation: "make_wires",
            message: e.what().to_string(),
        })?;
        let fixed = ffi::shape_fix::ShapeFix_Wire_Wire(&fix).map_err(|e| OcctError::Exception {
            operation: "make_wires",
            message: e.what().to_string(),
        })?;
        wires.push(fixed);
    }

    Ok(compound_of_shapes(
        &wires
            .iter()
            .map(|wire| ffi::topo_ds::TopoDS_Shape_to_owned(ffi::topo_ds::cast_wire_to_shape(wire)))
            .collect::<Vec<_>>(),
    ))
}

/// `Shape.removeInternalWires(minimumArea)`: a `ShapeUpgrade_RemoveInternalWires`
/// run, returning both the result shape and `Perform`'s flag.
pub(crate) fn remove_internal_wires(
    shape: &TopoDS_Shape,
    minimum_area: f64,
) -> Result<(UniquePtr<TopoDS_Shape>, bool), OcctError> {
    if shape.IsNull() || !minimum_area.is_finite() {
        return Err(OcctError::NotDone {
            operation: "remove_internal_wires",
        });
    }
    let mut fix = ffi::shape_upgrade::RemoveInternalWires_new(shape).map_err(|e| {
        OcctError::Exception {
            operation: "remove_internal_wires",
            message: e.what().to_string(),
        }
    })?;
    ffi::shape_upgrade::RemoveInternalWires_set_min_area(fix.pin_mut(), minimum_area);
    let performed = ffi::shape_upgrade::RemoveInternalWires_Perform(fix.pin_mut()).map_err(|e| {
        OcctError::Exception {
            operation: "remove_internal_wires",
            message: e.what().to_string(),
        }
    })?;
    let result = ffi::shape_upgrade::RemoveInternalWires_GetResult(&fix).map_err(|e| {
        OcctError::Exception {
            operation: "remove_internal_wires",
            message: e.what().to_string(),
        }
    })?;
    if result.IsNull() {
        return Err(OcctError::NotDone {
            operation: "remove_internal_wires",
        });
    }
    Ok((result, performed))
}

/// A compound of the given shapes. One shape is returned as itself, so a
/// compound of a single face is not a compound.
fn compound_of_shapes(shapes: &[UniquePtr<TopoDS_Shape>]) -> UniquePtr<TopoDS_Shape> {
    if shapes.len() == 1 {
        return ffi::topo_ds::TopoDS_Shape_to_owned(&shapes[0]);
    }
    let mut compound = ffi::topo_ds::TopoDS_Compound_new();
    let builder = ffi::b_rep::BRep_Builder_new();
    let builder = ffi::b_rep::BRep_Builder_upcast_to_topods_builder(&builder);
    builder.MakeCompound(compound.pin_mut());
    let mut compound = ffi::topo_ds::TopoDS_Compound_as_shape(compound);
    for shape in shapes {
        builder.Add(compound.pin_mut(), shape);
    }
    compound
}

// ---------------------------------------------------------------------------
// `Shape.makeOffset2D` fill (`surface_ops.hxx`)
// ---------------------------------------------------------------------------

fn join_type(kind: i32) -> GeomAbs_JoinType {
    match kind {
        1 => GeomAbs_JoinType::GeomAbs_Tangent,
        2 => GeomAbs_JoinType::GeomAbs_Intersection,
        _ => GeomAbs_JoinType::GeomAbs_Arc,
    }
}

fn face_area(face: &TopoDS_Face) -> f64 {
    let mut props = ffi::g_prop::GProps_new();
    ffi::b_rep_g_prop::BRepGProp::SurfaceProperties(
        ffi::topo_ds::cast_face_to_shape(face),
        props.pin_mut(),
        false,
        false,
    );
    props.Mass().abs()
}

fn wire_area(wire: &TopoDS_Wire) -> f64 {
    let make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeFace_wire(wire, true);
    if !make.IsDone() {
        return 0.0;
    }
    face_area(make.Face())
}

fn collect_edges(wire: &TopoDS_Wire, edges: &mut Vec<UniquePtr<TopoDS_Edge>>) {
    let mut explorer = ffi::b_rep_tools::BRepTools_WireExplorer_new(wire);
    while explorer.More() {
        let edge = explorer.Current();
        if explorer.Orientation() == TopAbs_Orientation::TopAbs_REVERSED {
            let reversed = ffi::topo_ds::Reversed(ffi::topo_ds::cast_edge_to_shape(edge));
            edges.push(ffi::topo_ds::TopoDS_Edge_to_owned(TopoDS::Edge(&reversed)));
        } else {
            edges.push(ffi::topo_ds::TopoDS_Edge_to_owned(edge));
        }
        explorer.pin_mut().Next();
    }
}

fn wire_point(edges: &[UniquePtr<TopoDS_Edge>]) -> UniquePtr<ffi::gp::gp_Pnt> {
    if edges.is_empty() {
        return ffi::gp::new_point(0.0, 0.0, 0.0);
    }
    let curve = ffi::b_rep_adaptor::BRepAdaptor_Curve_new(&edges[0]);
    let u = 0.5 * (curve.FirstParameter() + curve.LastParameter());
    ffi::b_rep_adaptor::BRepAdaptor_Curve_value(&curve, u)
}

fn wire_ends(
    edges: &[UniquePtr<TopoDS_Edge>],
    first: &mut UniquePtr<TopoDS_Vertex>,
    last: &mut UniquePtr<TopoDS_Vertex>,
) -> bool {
    if edges.is_empty() {
        return false;
    }
    let mut other = ffi::topo_ds::TopoDS_Vertex_new();
    ffi::top_exp::TopExp::EdgeVertices(&edges[0], first.pin_mut(), other.pin_mut(), true);
    let mut other2 = ffi::topo_ds::TopoDS_Vertex_new();
    ffi::top_exp::TopExp::EdgeVertices(
        edges.last().unwrap(),
        other2.pin_mut(),
        last.pin_mut(),
        true,
    );
    !ffi::topo_ds::cast_vertex_to_shape(first).IsNull()
        && !ffi::topo_ds::cast_vertex_to_shape(last).IsNull()
}

fn face_contains(face: &TopoDS_Face, point: &ffi::gp::gp_Pnt) -> bool {
    ffi::b_rep_class::BRepClass_FaceClassifier_State_point(
        face,
        point.X(),
        point.Y(),
        point.Z(),
        ffi::precision::Precision::Confusion(),
    )
    .map(|state| state == TopAbs_State::TopAbs_IN)
    .unwrap_or(false)
}

fn build_face(
    outer: &TopoDS_Wire,
    holes: &[UniquePtr<TopoDS_Wire>],
    reverse_holes: bool,
) -> UniquePtr<TopoDS_Face> {
    let mut make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeFace_wire(outer, true);
    if !make.IsDone() {
        return ffi::topo_ds::TopoDS_Face_new();
    }
    for hole in holes {
        if reverse_holes {
            let reversed = ffi::topo_ds::Reversed(ffi::topo_ds::cast_wire_to_shape(hole));
            make.pin_mut().add_wire(TopoDS::Wire(&reversed));
        } else {
            make.pin_mut().add_wire(hole);
        }
    }
    if !make.IsDone() {
        return ffi::topo_ds::TopoDS_Face_new();
    }
    ffi::topo_ds::TopoDS_Face_to_owned(make.Face())
}

fn oriented_face(
    outer: &TopoDS_Wire,
    holes: &[UniquePtr<TopoDS_Wire>],
) -> UniquePtr<TopoDS_Face> {
    if holes.is_empty() {
        return build_face(outer, holes, false);
    }
    let mut expected = wire_area(outer);
    for hole in holes {
        expected -= wire_area(hole);
    }
    expected = expected.abs();
    let reversed = build_face(outer, holes, true);
    let plain = build_face(outer, holes, false);
    if ffi::topo_ds::cast_face_to_shape(&reversed).IsNull() {
        return plain;
    }
    if ffi::topo_ds::cast_face_to_shape(&plain).IsNull() {
        return reversed;
    }
    let reverse_error = (face_area(&reversed) - expected).abs();
    let plain_error = (face_area(&plain) - expected).abs();
    if reverse_error <= plain_error {
        reversed
    } else {
        plain
    }
}

fn compound_of(shapes: &[UniquePtr<TopoDS_Shape>]) -> Option<UniquePtr<TopoDS_Shape>> {
    if shapes.is_empty() {
        return None;
    }
    if shapes.len() == 1 {
        return Some(ffi::topo_ds::TopoDS_Shape_to_owned(&shapes[0]));
    }
    let mut compound = ffi::topo_ds::TopoDS_Compound_new();
    let builder = ffi::b_rep::BRep_Builder_new();
    let builder = ffi::b_rep::BRep_Builder_upcast_to_topods_builder(&builder);
    builder.MakeCompound(compound.pin_mut());
    let mut compound = ffi::topo_ds::TopoDS_Compound_as_shape(compound);
    for shape in shapes {
        builder.Add(compound.pin_mut(), shape);
    }
    Some(compound)
}

fn filled_regions(wires: &[UniquePtr<TopoDS_Wire>]) -> Option<UniquePtr<TopoDS_Shape>> {
    struct Entry {
        wire: UniquePtr<TopoDS_Wire>,
        area: f64,
        point: UniquePtr<ffi::gp::gp_Pnt>,
    }
    let confusion = ffi::precision::Precision::Confusion();
    let mut entries: Vec<Entry> = Vec::new();
    for wire in wires {
        let area = wire_area(wire);
        if area <= confusion {
            continue;
        }
        let mut edges = Vec::new();
        collect_edges(wire, &mut edges);
        let point = wire_point(&edges);
        entries.push(Entry {
            wire: ffi::topo_ds::TopoDS_Wire_to_owned(wire),
            area,
            point,
        });
    }
    entries.sort_by(|a, b| {
        b.area
            .partial_cmp(&a.area)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    struct WireFace {
        outer: UniquePtr<TopoDS_Wire>,
        holes: Vec<UniquePtr<TopoDS_Wire>>,
        face: UniquePtr<TopoDS_Face>,
    }
    let mut faces: Vec<WireFace> = Vec::new();
    for entry in entries {
        let mut best: Option<usize> = None;
        let mut best_area = f64::MAX;
        for (index, face) in faces.iter().enumerate() {
            let area = face_area(&face.face);
            if area < best_area && face_contains(&face.face, &entry.point) {
                best = Some(index);
                best_area = area;
            }
        }
        if let Some(best) = best {
            faces[best].holes.push(entry.wire);
            let face = oriented_face(&faces[best].outer, &faces[best].holes);
            if ffi::topo_ds::cast_face_to_shape(&face).IsNull() {
                return None;
            }
            faces[best].face = face;
        } else {
            let face = oriented_face(&entry.wire, &[]);
            if ffi::topo_ds::cast_face_to_shape(&face).IsNull() {
                return None;
            }
            faces.push(WireFace {
                outer: entry.wire,
                holes: Vec::new(),
                face,
            });
        }
    }

    let shapes: Vec<UniquePtr<TopoDS_Shape>> = faces
        .into_iter()
        .map(|face| ffi::topo_ds::TopoDS_Shape_to_owned(ffi::topo_ds::cast_face_to_shape(&face.face)))
        .collect();
    compound_of(&shapes)
}

fn fill_closed(
    source: &[UniquePtr<TopoDS_Wire>],
    offset: &[UniquePtr<TopoDS_Wire>],
) -> Option<UniquePtr<TopoDS_Shape>> {
    let confusion = ffi::precision::Precision::Confusion();
    let mut wires: Vec<UniquePtr<TopoDS_Wire>> = Vec::new();
    for wire in source {
        if ffi::b_rep::BRep_Tool_IsClosed_shape(ffi::topo_ds::cast_wire_to_shape(wire))
            && wire_area(wire) > confusion
        {
            wires.push(ffi::topo_ds::TopoDS_Wire_to_owned(wire));
        }
    }
    for wire in offset {
        if ffi::b_rep::BRep_Tool_IsClosed_shape(ffi::topo_ds::cast_wire_to_shape(wire))
            && wire_area(wire) > confusion
        {
            wires.push(ffi::topo_ds::TopoDS_Wire_to_owned(wire));
        }
    }
    filled_regions(&wires)
}

fn open_band(source: &TopoDS_Wire, offset: &TopoDS_Wire) -> Option<UniquePtr<TopoDS_Face>> {
    let mut source_edges = Vec::new();
    let mut offset_edges = Vec::new();
    collect_edges(source, &mut source_edges);
    collect_edges(offset, &mut offset_edges);

    let mut source_first = ffi::topo_ds::TopoDS_Vertex_new();
    let mut source_last = ffi::topo_ds::TopoDS_Vertex_new();
    let mut offset_first = ffi::topo_ds::TopoDS_Vertex_new();
    let mut offset_last = ffi::topo_ds::TopoDS_Vertex_new();
    if !wire_ends(&source_edges, &mut source_first, &mut source_last)
        || !wire_ends(&offset_edges, &mut offset_first, &mut offset_last)
    {
        return None;
    }

    let start = ffi::b_rep::BRep_Tool_Pnt(&source_first);
    let forward = start.Distance(&ffi::b_rep::BRep_Tool_Pnt(&offset_first))
        <= start.Distance(&ffi::b_rep::BRep_Tool_Pnt(&offset_last));
    let offset_start = if forward { &offset_first } else { &offset_last };
    let offset_end = if forward { &offset_last } else { &offset_first };

    let mut make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeWire_new();
    for edge in &source_edges {
        make.pin_mut().add_edge(edge);
    }
    let mut connector = ffi::b_rep_builder_api::BRepBuilderAPI_MakeEdge_gp_Pnt_gp_Pnt(
        &ffi::b_rep::BRep_Tool_Pnt(&source_last),
        &ffi::b_rep::BRep_Tool_Pnt(offset_end),
    );
    make.pin_mut().add_edge(connector.pin_mut().Edge());
    if forward {
        for edge in offset_edges.iter().rev() {
            let reversed = ffi::topo_ds::Reversed(ffi::topo_ds::cast_edge_to_shape(edge));
            make.pin_mut().add_edge(TopoDS::Edge(&reversed));
        }
    } else {
        for edge in &offset_edges {
            make.pin_mut().add_edge(edge);
        }
    }
    let mut connector = ffi::b_rep_builder_api::BRepBuilderAPI_MakeEdge_gp_Pnt_gp_Pnt(
        &ffi::b_rep::BRep_Tool_Pnt(offset_start),
        &ffi::b_rep::BRep_Tool_Pnt(&source_first),
    );
    make.pin_mut().add_edge(connector.pin_mut().Edge());
    if !make.IsDone() {
        return None;
    }
    let face = ffi::b_rep_builder_api::BRepBuilderAPI_MakeFace_wire(make.pin_mut().Wire(), true);
    if !face.IsDone() {
        return None;
    }
    Some(ffi::topo_ds::TopoDS_Face_to_owned(face.Face()))
}

fn fill_open(
    source: &[UniquePtr<TopoDS_Wire>],
    offset: &[UniquePtr<TopoDS_Wire>],
) -> Option<UniquePtr<TopoDS_Shape>> {
    let mut open_source: Vec<UniquePtr<TopoDS_Wire>> = Vec::new();
    for wire in source {
        if !ffi::b_rep::BRep_Tool_IsClosed_shape(ffi::topo_ds::cast_wire_to_shape(wire)) {
            open_source.push(ffi::topo_ds::TopoDS_Wire_to_owned(wire));
        }
    }
    let mut open_offset: Vec<UniquePtr<TopoDS_Wire>> = Vec::new();
    for wire in offset {
        if !ffi::b_rep::BRep_Tool_IsClosed_shape(ffi::topo_ds::cast_wire_to_shape(wire)) {
            open_offset.push(ffi::topo_ds::TopoDS_Wire_to_owned(wire));
        }
    }
    if open_source.is_empty() || open_offset.is_empty() {
        return None;
    }

    let mut faces: Vec<UniquePtr<TopoDS_Shape>> = Vec::new();
    let mut used = vec![false; open_offset.len()];
    for source_wire in &open_source {
        let mut edges = Vec::new();
        collect_edges(source_wire, &mut edges);
        let mut first = ffi::topo_ds::TopoDS_Vertex_new();
        let mut last = ffi::topo_ds::TopoDS_Vertex_new();
        if !wire_ends(&edges, &mut first, &mut last) {
            continue;
        }
        let point = ffi::b_rep::BRep_Tool_Pnt(&first);
        let mut best: Option<usize> = None;
        let mut best_distance = f64::MAX;
        for (index, candidate) in open_offset.iter().enumerate() {
            if used[index] {
                continue;
            }
            let mut candidate_edges = Vec::new();
            collect_edges(candidate, &mut candidate_edges);
            let mut candidate_first = ffi::topo_ds::TopoDS_Vertex_new();
            let mut candidate_last = ffi::topo_ds::TopoDS_Vertex_new();
            if !wire_ends(&candidate_edges, &mut candidate_first, &mut candidate_last) {
                continue;
            }
            let distance = point.Distance(&ffi::b_rep::BRep_Tool_Pnt(&candidate_first));
            if distance < best_distance {
                best = Some(index);
                best_distance = distance;
            }
        }
        let Some(best) = best else {
            continue;
        };
        used[best] = true;
        let band = open_band(source_wire, &open_offset[best])?;
        if ffi::topo_ds::cast_face_to_shape(&band).IsNull() {
            return None;
        }
        faces.push(ffi::topo_ds::TopoDS_Shape_to_owned(ffi::topo_ds::cast_face_to_shape(&band)));
    }
    compound_of(&faces)
}

fn collect_source_wires(
    shape: &TopoDS_Shape,
    wires: &mut Vec<UniquePtr<TopoDS_Wire>>,
    have_faces: &mut bool,
    have_wires: &mut bool,
) -> bool {
    match shape.ShapeType() {
        TopAbs_ShapeEnum::TopAbs_FACE => {
            *have_faces = true;
            let mut explorer = ffi::top_exp::TopExp_Explorer_new(shape, TopAbs_ShapeEnum::TopAbs_WIRE);
            while explorer.More() {
                wires.push(ffi::topo_ds::TopoDS_Wire_to_owned(TopoDS::Wire(explorer.Current())));
                explorer.pin_mut().Next();
            }
            !wires.is_empty()
        }
        TopAbs_ShapeEnum::TopAbs_WIRE => {
            *have_wires = true;
            wires.push(ffi::topo_ds::TopoDS_Wire_to_owned(TopoDS::Wire(shape)));
            true
        }
        TopAbs_ShapeEnum::TopAbs_EDGE => {
            let mut make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeWire_new();
            make.pin_mut().add_edge(TopoDS::Edge(shape));
            if !make.IsDone() {
                return false;
            }
            *have_wires = true;
            wires.push(ffi::topo_ds::TopoDS_Wire_to_owned(make.pin_mut().Wire()));
            true
        }
        TopAbs_ShapeEnum::TopAbs_COMPOUND => {
            let mut any = false;
            let mut iterator = ffi::topo_ds::TopoDS_Iterator_new(shape);
            while iterator.More() {
                if !collect_source_wires(iterator.Value(), wires, have_faces, have_wires) {
                    return false;
                }
                any = true;
                iterator.pin_mut().Next();
            }
            any
        }
        _ => false,
    }
}

fn shapes_of_kind(shape: &TopoDS_Shape, kind: TopAbs_ShapeEnum) -> Vec<UniquePtr<TopoDS_Shape>> {
    let mut out = Vec::new();
    let mut explorer = ffi::top_exp::TopExp_Explorer_new(shape, kind);
    while explorer.More() {
        out.push(ffi::topo_ds::TopoDS_Shape_to_owned(explorer.Current()));
        explorer.pin_mut().Next();
    }
    out
}

/// `Shape.makeOffset2D(distance, join, fill, openResult)`: offset `shape` in its
/// plane, optionally filling the band between the source and the offset.
pub(crate) fn offset2d(
    shape: &TopoDS_Shape,
    distance: f64,
    kind: i32,
    fill: bool,
    open_result: bool,
) -> Result<UniquePtr<TopoDS_Shape>, OcctError> {
    if shape.IsNull() || !distance.is_finite() {
        return Err(OcctError::NotDone {
            operation: "offset2d",
        });
    }
    if fill && distance.abs() <= ffi::precision::Precision::Confusion() {
        return Err(OcctError::NotDone {
            operation: "offset2d",
        });
    }

    let mut source_wires: Vec<UniquePtr<TopoDS_Wire>> = Vec::new();
    let mut have_faces = false;
    let mut have_wires = false;
    if !collect_source_wires(shape, &mut source_wires, &mut have_faces, &mut have_wires)
        || source_wires.is_empty()
    {
        return Err(OcctError::NotDone {
            operation: "offset2d",
        });
    }
    if have_faces && have_wires {
        return Err(OcctError::NotDone {
            operation: "offset2d",
        });
    }

    let mut make = ffi::b_rep_offset_api::BRepOffsetAPI_MakeOffset_new();
    make.pin_mut().Init(join_type(kind), open_result);
    for wire in &source_wires {
        make.pin_mut().AddWire(wire);
    }
    make.pin_mut().Perform(distance, 0.0);
    if !make.IsDone() {
        return Err(OcctError::NotDone {
            operation: "offset2d",
        });
    }
    let offset = ffi::topo_ds::TopoDS_Shape_to_owned(make.pin_mut().Shape());
    if offset.IsNull() {
        return Err(OcctError::NotDone {
            operation: "offset2d",
        });
    }

    let mut offset_wires: Vec<UniquePtr<TopoDS_Wire>> = Vec::new();
    let mut explorer = ffi::top_exp::TopExp_Explorer_new(&offset, TopAbs_ShapeEnum::TopAbs_WIRE);
    while explorer.More() {
        offset_wires.push(ffi::topo_ds::TopoDS_Wire_to_owned(TopoDS::Wire(explorer.Current())));
        explorer.pin_mut().Next();
    }
    if offset_wires.is_empty() {
        return Err(OcctError::NotDone {
            operation: "offset2d",
        });
    }

    if !fill {
        if !have_faces {
            return Ok(offset);
        }
        return filled_regions(&offset_wires).ok_or(OcctError::NotDone {
            operation: "offset2d",
        });
    }

    let mut faces: Vec<UniquePtr<TopoDS_Shape>> = Vec::new();
    if let Some(closed) = fill_closed(&source_wires, &offset_wires) {
        faces.extend(shapes_of_kind(&closed, TopAbs_ShapeEnum::TopAbs_FACE));
    }
    if let Some(open) = fill_open(&source_wires, &offset_wires) {
        faces.extend(shapes_of_kind(&open, TopAbs_ShapeEnum::TopAbs_FACE));
    }
    compound_of(&faces).ok_or(OcctError::NotDone {
        operation: "offset2d",
    })
}

// ---------------------------------------------------------------------------
// `part_ops.hxx` FreeCAD behaviour
// ---------------------------------------------------------------------------

fn occt_exception(operation: &'static str, e: cxx::Exception) -> OcctError {
    OcctError::Exception {
        operation,
        message: e.what().to_string(),
    }
}

/// `BRep_Builder`'s in-place add: mutates `shape` so it gains `sub` as a child.
fn add_sub_shape(builder: &ffi::topo_ds::TopoDS_Builder, shape: &mut UniquePtr<TopoDS_Shape>, sub: &TopoDS_Shape) {
    builder.Add(shape.pin_mut(), sub);
}

/// Whether `shape` has any sub-shape of the given kind (`TopExp_Explorer`).
fn has_type(shape: &TopoDS_Shape, kind: TopAbs_ShapeEnum) -> bool {
    let xp = ffi::top_exp::TopExp_Explorer_new(shape, kind);
    xp.More()
}

/// Project `tool` onto `target` along `direction` (parallel) or from the
/// viewpoint `direction` (perspective).
pub(crate) fn project(
    tool: &TopoDS_Shape,
    target: &TopoDS_Shape,
    direction: Vector3,
    parallel: bool,
) -> Result<UniquePtr<TopoDS_Shape>, OcctError> {
    if tool.IsNull() || target.IsNull() {
        return Err(OcctError::InvalidInput {
            what: "project needs a tool and a target",
        });
    }
    let made = if parallel {
        if direction.x * direction.x + direction.y * direction.y + direction.z * direction.z
            <= ffi::precision::Precision::Confusion()
        {
            return Err(OcctError::NotDone {
                operation: "project",
            });
        }
        let dir = ffi::gp::gp_Dir_new(direction.x, direction.y, direction.z);
        ffi::b_rep_proj::BRepProj_Projection_dir_new(tool, target, &dir)
            .map_err(|e| occt_exception("project", e))?
    } else {
        let pnt = ffi::gp::new_point(direction.x, direction.y, direction.z);
        ffi::b_rep_proj::BRepProj_Projection_pnt_new(tool, target, &pnt)
            .map_err(|e| occt_exception("project", e))?
    };
    if !made.IsDone() {
        return Err(OcctError::NotDone {
            operation: "project",
        });
    }
    let compound = ffi::b_rep_proj::BRepProj_Projection_Shape(&made);
    Ok(ffi::topo_ds::TopoDS_Compound_as_shape(compound))
}

/// `Shape.defeaturing(faces)`: remove the faces and heal the gap.
pub(crate) fn defeature(
    shape: &TopoDS_Shape,
    faces: &TopTools_ListOfShape,
) -> Result<UniquePtr<TopoDS_Shape>, OcctError> {
    if shape.IsNull() || faces.IsEmpty() {
        return Err(OcctError::NotDone {
            operation: "defeaturing",
        });
    }
    let mut defeaturing = ffi::b_rep_algo_api::BRepAlgoAPI_Defeaturing_new();
    ffi::b_rep_algo_api::BRepAlgoAPI_Defeaturing_SetRunParallel(defeaturing.pin_mut(), true);
    ffi::b_rep_algo_api::BRepAlgoAPI_Defeaturing_SetToFillHistory(defeaturing.pin_mut(), true);
    ffi::b_rep_algo_api::BRepAlgoAPI_Defeaturing_SetShape(defeaturing.pin_mut(), shape);
    let mut it = ffi::top_tools::TopTools_ListIteratorOfListOfShape_new(faces);
    while it.More() {
        let face = it.Value();
        if face.ShapeType() != TopAbs_ShapeEnum::TopAbs_FACE {
            return Err(OcctError::InvalidInput {
                what: "defeaturing removes faces",
            });
        }
        ffi::b_rep_algo_api::BRepAlgoAPI_Defeaturing_AddFaceToRemove(defeaturing.pin_mut(), face);
        it.pin_mut().Next();
    }
    ffi::b_rep_algo_api::BRepAlgoAPI_Defeaturing_Build(defeaturing.pin_mut())
        .map_err(|e| occt_exception("defeaturing", e))?;
    if !ffi::b_rep_algo_api::BRepAlgoAPI_Defeaturing_IsDone(&defeaturing) {
        return Err(OcctError::NotDone {
            operation: "defeaturing",
        });
    }
    let result = ffi::b_rep_algo_api::BRepAlgoAPI_Defeaturing_Shape(defeaturing.pin_mut());
    if result.IsNull() {
        return Err(OcctError::NotDone {
            operation: "defeaturing",
        });
    }
    Ok(result)
}

/// `Face.validate()`: repair each wire, then fall back to `ShapeFix_Shape`.
pub(crate) fn validate_face(
    shape: &TopoDS_Shape,
) -> Result<UniquePtr<TopoDS_Shape>, OcctError> {
    if shape.IsNull() || shape.ShapeType() != TopAbs_ShapeEnum::TopAbs_FACE {
        return Err(OcctError::InvalidInput {
            what: "validate_face expects a face",
        });
    }
    let face = TopoDS::Face(shape);
    let checker = ffi::b_rep_check::BRepCheck_Analyzer_new(shape, true);
    if checker.IsValid() {
        return Ok(ffi::topo_ds::TopoDS_Shape_to_owned(shape));
    }

    let outer = ffi::b_rep_tools::outer_wire(face);
    let mut seen = ffi::top_tools::new_indexed_map_of_shape();
    let _ = seen.pin_mut().Add(ffi::topo_ds::cast_wire_to_shape(&outer));

    let fix = ffi::shape_fix::ShapeFix_Wire_new();
    ffi::shape_fix::ShapeFix_Wire_SetFace(&fix, face).map_err(|e| occt_exception("validate_face", e))?;
    ffi::shape_fix::ShapeFix_Wire_Load(&fix, &outer).map_err(|e| occt_exception("validate_face", e))?;
    ffi::shape_fix::ShapeFix_Wire_Perform(&fix).map_err(|e| occt_exception("validate_face", e))?;
    let first = ffi::shape_fix::ShapeFix_Wire_WireAPIMake(&fix)
        .map_err(|e| occt_exception("validate_face", e))?;
    let mut mk_face = ffi::b_rep_builder_api::BRepBuilderAPI_MakeFace_wire(&first, false);

    let mut xp = ffi::top_exp::TopExp_Explorer_new(shape, TopAbs_ShapeEnum::TopAbs_WIRE);
    while xp.More() {
        if seen.FindIndex(xp.Current()) == 0 {
            ffi::shape_fix::ShapeFix_Wire_Load(&fix, TopoDS::Wire(xp.Current()))
                .map_err(|e| occt_exception("validate_face", e))?;
            ffi::shape_fix::ShapeFix_Wire_Perform(&fix)
                .map_err(|e| occt_exception("validate_face", e))?;
            let fixed = ffi::shape_fix::ShapeFix_Wire_WireAPIMake(&fix)
                .map_err(|e| occt_exception("validate_face", e))?;
            mk_face.pin_mut().add_wire(&fixed);
        }
        xp.pin_mut().Next();
    }

    let face_shape = ffi::topo_ds::cast_face_to_shape(mk_face.Face());
    let checker = ffi::b_rep_check::BRepCheck_Analyzer_new(face_shape, true);
    if checker.IsValid() {
        return Ok(ffi::topo_ds::TopoDS_Shape_to_owned(face_shape));
    }

    let fixer = ffi::shape_fix::ShapeFix_Shape_from_shape(face_shape)
        .map_err(|e| occt_exception("validate_face", e))?;
    let confusion = ffi::precision::Precision::Confusion();
    ffi::shape_fix::ShapeFix_Root_set_precision(
        ffi::shape_fix::ShapeFix_Shape_as_root(&fixer),
        confusion,
    );
    ffi::shape_fix::ShapeFix_Root_set_max_tolerance(
        ffi::shape_fix::ShapeFix_Shape_as_root(&fixer),
        confusion,
    );
    ffi::shape_fix::ShapeFix_Shape_Perform(&fixer).map_err(|e| occt_exception("validate_face", e))?;
    let wire_tool = ffi::shape_fix::ShapeFix_Shape_FixWireTool(&fixer);
    if !wire_tool.IsNull() {
        ffi::shape_fix::ShapeFix_Wire_Perform(&wire_tool).map_err(|e| occt_exception("validate_face", e))?;
    }
    let face_tool = ffi::shape_fix::ShapeFix_Shape_FixFaceTool(&fixer);
    if !face_tool.IsNull() {
        ffi::shape_fix::ShapeFix_Face_Perform(&face_tool).map_err(|e| occt_exception("validate_face", e))?;
    }
    ffi::shape_fix::ShapeFix_Shape_Shape(&fixer).map_err(|e| occt_exception("validate_face", e))
}

/// `Shape.fix(precision, mintol, maxtol)`: `ShapeFix_Shape` with upstream's
/// per-kind tool walk.
pub(crate) fn fix_shape(
    shape: &TopoDS_Shape,
    precision: f64,
    min_tolerance: f64,
    max_tolerance: f64,
) -> Result<UniquePtr<TopoDS_Shape>, OcctError> {
    if shape.IsNull() {
        return Err(OcctError::NotDone {
            operation: "fix_shape",
        });
    }
    let kind = shape.ShapeType();
    let fix = ffi::shape_fix::ShapeFix_Shape_from_shape(shape)
        .map_err(|e| occt_exception("fix_shape", e))?;
    ffi::shape_fix::ShapeFix_Root_set_precision(
        ffi::shape_fix::ShapeFix_Shape_as_root(&fix),
        precision,
    );
    ffi::shape_fix::ShapeFix_Root_set_min_tolerance(
        ffi::shape_fix::ShapeFix_Shape_as_root(&fix),
        min_tolerance,
    );
    ffi::shape_fix::ShapeFix_Root_set_max_tolerance(
        ffi::shape_fix::ShapeFix_Shape_as_root(&fix),
        max_tolerance,
    );
    ffi::shape_fix::ShapeFix_Shape_Perform(&fix).map_err(|e| occt_exception("fix_shape", e))?;

    if kind == TopAbs_ShapeEnum::TopAbs_SOLID {
        let wire_tool = ffi::shape_fix::ShapeFix_Shape_FixWireTool(&fix);
        if !wire_tool.IsNull() {
            ffi::shape_fix::ShapeFix_Wire_Perform(&wire_tool).map_err(|e| occt_exception("fix_shape", e))?;
        }
        let face_tool = ffi::shape_fix::ShapeFix_Shape_FixFaceTool(&fix);
        if !face_tool.IsNull() {
            ffi::shape_fix::ShapeFix_Face_Perform(&face_tool).map_err(|e| occt_exception("fix_shape", e))?;
        }
        let shell_tool = ffi::shape_fix::ShapeFix_Shape_FixShellTool(&fix);
        if !shell_tool.IsNull() {
            ffi::shape_fix::ShapeFix_Shell_Perform(&shell_tool).map_err(|e| occt_exception("fix_shape", e))?;
        }
        let solid_tool = ffi::shape_fix::ShapeFix_Shape_FixSolidTool(&fix);
        if !solid_tool.IsNull() {
            ffi::shape_fix::ShapeFix_Solid_Perform(&solid_tool).map_err(|e| occt_exception("fix_shape", e))?;
            let tool_shape = ffi::shape_fix::ShapeFix_Solid_Shape(&solid_tool)
                .map_err(|e| occt_exception("fix_shape", e))?;
            if !tool_shape.IsNull() {
                return Ok(tool_shape);
            }
        }
    } else if kind == TopAbs_ShapeEnum::TopAbs_SHELL {
        let wire_tool = ffi::shape_fix::ShapeFix_Shape_FixWireTool(&fix);
        if !wire_tool.IsNull() {
            ffi::shape_fix::ShapeFix_Wire_Perform(&wire_tool).map_err(|e| occt_exception("fix_shape", e))?;
        }
        let face_tool = ffi::shape_fix::ShapeFix_Shape_FixFaceTool(&fix);
        if !face_tool.IsNull() {
            ffi::shape_fix::ShapeFix_Face_Perform(&face_tool).map_err(|e| occt_exception("fix_shape", e))?;
        }
        let shell_tool = ffi::shape_fix::ShapeFix_Shape_FixShellTool(&fix);
        if !shell_tool.IsNull() {
            ffi::shape_fix::ShapeFix_Shell_Perform(&shell_tool).map_err(|e| occt_exception("fix_shape", e))?;
            let tool_shape = ffi::shape_fix::ShapeFix_Shell_Shape(&shell_tool)
                .map_err(|e| occt_exception("fix_shape", e))?;
            if !tool_shape.IsNull() {
                return Ok(tool_shape);
            }
        }
    } else if kind == TopAbs_ShapeEnum::TopAbs_FACE {
        let wire_tool = ffi::shape_fix::ShapeFix_Shape_FixWireTool(&fix);
        if !wire_tool.IsNull() {
            ffi::shape_fix::ShapeFix_Wire_Perform(&wire_tool).map_err(|e| occt_exception("fix_shape", e))?;
        }
        let face_tool = ffi::shape_fix::ShapeFix_Shape_FixFaceTool(&fix);
        if !face_tool.IsNull() {
            ffi::shape_fix::ShapeFix_Face_Perform(&face_tool).map_err(|e| occt_exception("fix_shape", e))?;
        }
    } else if kind == TopAbs_ShapeEnum::TopAbs_WIRE {
        let wire_tool = ffi::shape_fix::ShapeFix_Shape_FixWireTool(&fix);
        if !wire_tool.IsNull() {
            ffi::shape_fix::ShapeFix_Wire_Perform(&wire_tool).map_err(|e| occt_exception("fix_shape", e))?;
        }
    }
    ffi::shape_fix::ShapeFix_Shape_Shape(&fix).map_err(|e| occt_exception("fix_shape", e))
}

/// The pcurves of `wire`'s edges on `surface` (see the `fc_wire_on_surface`
/// contract): updated in place, or a located copy flattened to the global frame.
fn wire_on_surface(
    wire: &TopoDS_Wire,
    surface: &ffi::geom::Handle_Geom_Surface,
) -> Result<UniquePtr<TopoDS_Wire>, OcctError> {
    let located = !ffi::topo_ds::cast_wire_to_shape(wire).Location().IsIdentity();
    let mut target = ffi::topo_ds::TopoDS_Wire_to_owned(wire);
    if located {
        let mut copier = ffi::b_rep_builder_api::BRepBuilderAPI_Copy_new(
            ffi::topo_ds::cast_wire_to_shape(wire),
            true,
            false,
        );
        if !copier.IsDone()
            || copier.pin_mut().Shape().ShapeType() != TopAbs_ShapeEnum::TopAbs_WIRE
        {
            return Err(OcctError::NotDone {
                operation: "wire_on_surface",
            });
        }
        target = ffi::topo_ds::TopoDS_Wire_to_owned(TopoDS::Wire(copier.pin_mut().Shape()));
        let identity = ffi::top_loc::Location_new();
        target.pin_mut().set_location(&identity, false);
    }

    let builder = ffi::b_rep::BRep_Builder_new();
    let mut xp = ffi::top_exp::TopExp_Explorer_new(
        ffi::topo_ds::cast_wire_to_shape(&target),
        TopAbs_ShapeEnum::TopAbs_EDGE,
    );
    let confusion = ffi::precision::Precision::Confusion();
    while xp.More() {
        let edge = TopoDS::Edge(xp.Current());
        let mut first = 0.0;
        let mut last = 0.0;
        let curve = ffi::b_rep::BRep_Tool_Curve(edge, &mut first, &mut last);
        if curve.IsNull() {
            xp.pin_mut().Next();
            continue;
        }
        let piece = ffi::geom::Geom_TrimmedCurve_new(&curve, first, last)
            .map_err(|e| occt_exception("wire_on_surface", e))?;
        let piece_curve = ffi::geom::new_HandleGeomCurve_from_HandleGeom_TrimmedCurve(&piece);
        let pcurve = ffi::geom_proj_lib::GeomProjLib_Curve2d(&piece_curve, first, last, surface)
            .map_err(|e| occt_exception("wire_on_surface", e))?;
        if pcurve.IsNull() {
            return Err(OcctError::NotDone {
                operation: "wire_on_surface",
            });
        }
        let identity = ffi::top_loc::Location_new();
        ffi::b_rep::BRep_Builder_UpdateEdge(&builder, edge, &pcurve, surface, &identity, confusion);
        ffi::b_rep_lib::BRepLib_SameRange(edge, confusion)
            .map_err(|e| occt_exception("wire_on_surface", e))?;
        xp.pin_mut().Next();
    }
    Ok(target)
}

/// `Part.Face(wire)` for one boundary wire.
pub(crate) fn face_from_wire(
    wire: &TopoDS_Shape,
) -> Result<UniquePtr<TopoDS_Shape>, OcctError> {
    if wire.IsNull() || wire.ShapeType() != TopAbs_ShapeEnum::TopAbs_WIRE {
        return Err(OcctError::InvalidInput {
            what: "face_from_wire expects a wire",
        });
    }
    let w = TopoDS::Wire(wire);
    let make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeFace_wire(w, false);
    if make.IsDone() && !ffi::topo_ds::cast_face_to_shape(make.Face()).IsNull() {
        return Ok(ffi::topo_ds::TopoDS_Shape_to_owned(
            ffi::topo_ds::cast_face_to_shape(make.Face()),
        ));
    }

    let first_edge = ffi::top_exp::TopExp_Explorer_new(wire, TopAbs_ShapeEnum::TopAbs_EDGE);
    if !first_edge.More() {
        return Err(OcctError::NotDone {
            operation: "face_from_wire",
        });
    }
    let mut pcurve = ffi::geom2d::Handle_Geom2d_Curve_new();
    let mut support = ffi::geom::Handle_Geom_Surface_new();
    let mut location = ffi::top_loc::Location_new();
    let mut first = 0.0;
    let mut last = 0.0;
    ffi::b_rep::BRep_Tool_CurveOnSurface(
        TopoDS::Edge(first_edge.Current()),
        pcurve.pin_mut(),
        support.pin_mut(),
        location.pin_mut(),
        &mut first,
        &mut last,
    );
    if pcurve.IsNull() || support.IsNull() {
        return Err(OcctError::NotDone {
            operation: "face_from_wire",
        });
    }

    let mut copier = ffi::b_rep_builder_api::BRepBuilderAPI_Copy_new(wire, true, false);
    if !copier.IsDone() || copier.pin_mut().Shape().ShapeType() != TopAbs_ShapeEnum::TopAbs_WIRE {
        return Err(OcctError::NotDone {
            operation: "face_from_wire",
        });
    }
    let projected = wire_on_surface(TopoDS::Wire(copier.pin_mut().Shape()), &support)?;

    let on_wire = ffi::b_rep_builder_api::BRepBuilderAPI_MakeFace_wire(&projected, false);
    if on_wire.IsDone() && !ffi::topo_ds::cast_face_to_shape(on_wire.Face()).IsNull() {
        return Ok(ffi::topo_ds::TopoDS_Shape_to_owned(
            ffi::topo_ds::cast_face_to_shape(on_wire.Face()),
        ));
    }
    let mut on_surface = ffi::b_rep_builder_api::BRepBuilderAPI_MakeFace_surface(
        &support,
        ffi::precision::Precision::Confusion(),
    );
    if !on_surface.IsDone() {
        return Err(OcctError::NotDone {
            operation: "face_from_wire",
        });
    }
    on_surface.pin_mut().add_wire(&projected);
    if !on_surface.IsDone() || ffi::topo_ds::cast_face_to_shape(on_surface.Face()).IsNull() {
        return Err(OcctError::NotDone {
            operation: "face_from_wire",
        });
    }
    let check = ffi::b_rep_check::BRepCheck_Analyzer_new(
        ffi::topo_ds::cast_face_to_shape(on_surface.Face()),
        true,
    );
    if !check.IsValid() {
        return Err(OcctError::NotDone {
            operation: "face_from_wire",
        });
    }
    Ok(ffi::topo_ds::TopoDS_Shape_to_owned(
        ffi::topo_ds::cast_face_to_shape(on_surface.Face()),
    ))
}

/// Attach a further boundary wire to an existing face (`BRepBuilderAPI_MakeFace::Add`).
pub(crate) fn face_add_wire(
    face: &TopoDS_Shape,
    wire: &TopoDS_Shape,
) -> Result<UniquePtr<TopoDS_Shape>, OcctError> {
    if face.IsNull()
        || face.ShapeType() != TopAbs_ShapeEnum::TopAbs_FACE
        || wire.IsNull()
        || wire.ShapeType() != TopAbs_ShapeEnum::TopAbs_WIRE
    {
        return Err(OcctError::InvalidInput {
            what: "face_add_wire expects a face and a wire",
        });
    }
    let mut face_shape = ffi::topo_ds::TopoDS_Shape_to_owned(face);
    let builder = ffi::b_rep::BRep_Builder_new();
    let builder = ffi::b_rep::BRep_Builder_upcast_to_topods_builder(&builder);
    builder.Add(face_shape.pin_mut(), wire);
    Ok(face_shape)
}

/// `Part.Face(surface, wires)` for an elementary surface described by `spec`.
pub(crate) fn make_face_on_surface(
    spec: &SurfaceSpec,
    wires: &TopTools_ListOfShape,
) -> Result<UniquePtr<TopoDS_Shape>, OcctError> {
    if wires.IsEmpty() {
        return Err(OcctError::InvalidInput {
            what: "a face needs a wire",
        });
    }
    let (origin, axis, x_dir, direct) = match spec {
        SurfaceSpec::Plane {
            origin,
            normal,
            x_dir,
            direct,
        } => (*origin, *normal, *x_dir, *direct),
        SurfaceSpec::Cylinder {
            origin,
            axis,
            x_dir,
            direct,
            ..
        } => (*origin, *axis, *x_dir, *direct),
        SurfaceSpec::Cone {
            origin,
            axis,
            x_dir,
            direct,
            ..
        } => (*origin, *axis, *x_dir, *direct),
        SurfaceSpec::Sphere {
            center,
            axis,
            x_dir,
            direct,
            ..
        } => (*center, *axis, *x_dir, *direct),
        SurfaceSpec::Torus {
            center,
            axis,
            x_dir,
            direct,
            ..
        } => (*center, *axis, *x_dir, *direct),
    };

    let mut frame = ffi::gp::gp_Ax3_new3(
        &ffi::gp::new_point(origin.x, origin.y, origin.z),
        &ffi::gp::gp_Dir_new(axis.x, axis.y, axis.z),
        &ffi::gp::gp_Dir_new(x_dir.x, x_dir.y, x_dir.z),
    )
    .map_err(|e| occt_exception("make_face_on_surface", e))?;
    if !direct {
        frame.pin_mut().YReverse();
    }

    let surface: UniquePtr<ffi::geom::Handle_Geom_Surface> = match spec {
        SurfaceSpec::Plane { .. } => {
            let plane = ffi::gp::gp_Pln_new_frame(&frame)
                .map_err(|e| occt_exception("make_face_on_surface", e))?;
            let handle = ffi::geom::Geom_Plane_new(&plane);
            ffi::geom::plane_to_surface(&handle)
        }
        SurfaceSpec::Cylinder { radius, .. } => {
            let handle = ffi::geom::Geom_CylindricalSurface_new(&frame, *radius);
            ffi::geom::cylinder_to_surface(&handle)
        }
        SurfaceSpec::Cone {
            radius, semi_angle, ..
        } => {
            let handle = ffi::geom::Geom_ConicalSurface_new(&frame, *semi_angle, *radius)
                .map_err(|e| occt_exception("make_face_on_surface", e))?;
            ffi::geom::conical_to_surface(&handle)
        }
        SurfaceSpec::Sphere { radius, .. } => {
            let handle = ffi::geom::Geom_SphericalSurface_new(&frame, *radius)
                .map_err(|e| occt_exception("make_face_on_surface", e))?;
            ffi::geom::spherical_to_surface(&handle)
        }
        SurfaceSpec::Torus {
            major_radius,
            minor_radius,
            ..
        } => {
            let handle = ffi::geom::Geom_ToroidalSurface_new(&frame, *major_radius, *minor_radius)
                .map_err(|e| occt_exception("make_face_on_surface", e))?;
            ffi::geom::toroidal_to_surface(&handle)
        }
    };

    let mut it = ffi::top_tools::TopTools_ListIteratorOfListOfShape_new(wires);
    if !it.More() || it.Value().ShapeType() != TopAbs_ShapeEnum::TopAbs_WIRE {
        return Err(OcctError::InvalidInput {
            what: "make_face_on_surface expects wires",
        });
    }
    let outer = wire_on_surface(TopoDS::Wire(it.Value()), &surface)?;
    let mut make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeFace_surface_wire(&surface, &outer, true)
        .map_err(|e| occt_exception("make_face_on_surface", e))?;
    if !make.IsDone() {
        return Err(OcctError::NotDone {
            operation: "make_face_on_surface",
        });
    }
    it.pin_mut().Next();
    while it.More() {
        if it.Value().ShapeType() != TopAbs_ShapeEnum::TopAbs_WIRE {
            return Err(OcctError::InvalidInput {
                what: "make_face_on_surface expects wires",
            });
        }
        let inner = wire_on_surface(TopoDS::Wire(it.Value()), &surface)?;
        make.pin_mut().add_wire(&inner);
        if !make.IsDone() {
            return Err(OcctError::NotDone {
                operation: "make_face_on_surface",
            });
        }
        it.pin_mut().Next();
    }
    if ffi::topo_ds::cast_face_to_shape(make.Face()).IsNull() {
        return Err(OcctError::NotDone {
            operation: "make_face_on_surface",
        });
    }
    Ok(ffi::topo_ds::TopoDS_Shape_to_owned(
        ffi::topo_ds::cast_face_to_shape(make.Face()),
    ))
}

/// `TopoDS_Iterator`-based `fc_bullseye_wire_direction`.
fn bullseye_wire_direction(plane: &ffi::gp::gp_Pln, wire: &TopoDS_Wire) -> Result<i32, OcctError> {
    let make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeFace_Plane_wire(plane, wire, true)
        .map_err(|e| occt_exception("bullseye", e))?;
    if !make.IsDone() {
        return Ok(0);
    }
    let face = make.Face();
    let it = ffi::topo_ds::TopoDS_Iterator_new_cum_orientation(
        ffi::topo_ds::cast_face_to_shape(face),
        false,
    );
    if ffi::topo_ds::cast_face_to_shape(face).IsNull() || !it.More() {
        return Ok(0);
    }
    Ok(if it.Value().Orientation() == ffi::topo_ds::cast_wire_to_shape(wire).Orientation() {
        1
    } else {
        -1
    })
}

/// The `(u, v)` of a wire's first vertex on `plane`, with its tolerance.
fn bullseye_sample(
    wire: &TopoDS_Shape,
    plane: &ffi::gp::gp_Pln,
) -> Option<(UniquePtr<ffi::gp::gp_Pnt2d>, f64)> {
    let xp = ffi::top_exp::TopExp_Explorer_new(wire, TopAbs_ShapeEnum::TopAbs_VERTEX);
    if !xp.More() {
        return None;
    }
    let vertex = TopoDS::Vertex(xp.Current());
    let tolerance = ffi::b_rep::BRep_Tool_Tolerance_vertex(vertex);
    let surface = {
        let handle = ffi::geom::Geom_Plane_new(plane);
        ffi::geom::plane_to_surface(&handle)
    };
    let projection = ffi::geom_api::GeomAPI_ProjectPointOnSurf_new(
        &ffi::b_rep::BRep_Tool_Pnt(vertex),
        &surface,
    );
    if projection.NbPoints() < 1 {
        return None;
    }
    let mut u = 0.0;
    let mut v = 0.0;
    projection.LowerDistanceParameters(&mut u, &mut v);
    Some((ffi::gp::new_point_2d(u, v), tolerance))
}

/// `Part.makeFace(wires, "Part::FaceMakerBullseye")`.
pub(crate) fn bullseye_face(
    wires: &TopTools_ListOfShape,
) -> Result<UniquePtr<TopoDS_Shape>, OcctError> {
    if wires.IsEmpty() {
        return Err(OcctError::InvalidInput {
            what: "bullseye_face needs wires",
        });
    }
    let mut it = ffi::top_tools::TopTools_ListIteratorOfListOfShape_new(wires);
    while it.More() {
        if it.Value().ShapeType() != TopAbs_ShapeEnum::TopAbs_WIRE
            || !ffi::b_rep::BRep_Tool_IsClosed_shape(it.Value())
        {
            return Err(OcctError::InvalidInput {
                what: "bullseye_face needs closed wires",
            });
        }
        it.pin_mut().Next();
    }

    let builder = ffi::b_rep::BRep_Builder_new();
    let builder_upcast = ffi::b_rep::BRep_Builder_upcast_to_topods_builder(&builder);
    let mut compound = ffi::topo_ds::TopoDS_Compound_new();
    builder_upcast.MakeCompound(compound.pin_mut());
    let mut compound_shape = ffi::topo_ds::TopoDS_Compound_as_shape(compound);
    let mut it = ffi::top_tools::TopTools_ListIteratorOfListOfShape_new(wires);
    while it.More() {
        let mut copy = ffi::b_rep_builder_api::BRepBuilderAPI_Copy_new(it.Value(), true, false);
        builder_upcast.Add(compound_shape.pin_mut(), copy.pin_mut().Shape());
        it.pin_mut().Next();
    }

    let finder = ffi::b_rep_lib::BRepLib_FindSurface_new(&compound_shape, -1.0, true);
    if !finder.Found() {
        return Err(OcctError::NotDone {
            operation: "bullseye_face",
        });
    }
    let found_surface = ffi::b_rep_lib::BRepLib_FindSurface_Surface(&finder);
    let plane_handle = ffi::geom::new_HandleGeomPlane_from_HandleGeomSurface(&found_surface);
    if plane_handle.IsNull() {
        return Err(OcctError::NotDone {
            operation: "bullseye_face",
        });
    }
    let plane = ffi::geom::Handle_Geom_Plane_Pln(&plane_handle);
    let surface = ffi::geom::plane_to_surface(&plane_handle);

    struct Item {
        wire: UniquePtr<TopoDS_Wire>,
        extent: f64,
    }
    let mut items: Vec<Item> = Vec::new();
    let mut it = ffi::top_tools::TopTools_ListIteratorOfListOfShape_new(wires);
    while it.More() {
        let mut box_ = ffi::bnd::Bnd_Box_new();
        ffi::b_rep_bnd_lib::BRepBndLib::AddOptimal(it.Value(), box_.pin_mut(), false, false);
        if !box_.IsVoid() {
            items.push(Item {
                wire: ffi::topo_ds::TopoDS_Wire_to_owned(TopoDS::Wire(it.Value())),
                extent: box_.SquareExtent(),
            });
        }
        it.pin_mut().Next();
    }
    items.sort_by(|a, b| b.extent.partial_cmp(&a.extent).unwrap_or(std::cmp::Ordering::Equal));

    struct Driller {
        face: UniquePtr<TopoDS_Face>,
        bound: UniquePtr<TopoDS_Face>,
    }
    let confusion = ffi::precision::Precision::Confusion();
    let mut faces: Vec<Driller> = Vec::new();
    for item in &items {
        let Some((uv, tolerance)) = bullseye_sample(ffi::topo_ds::cast_wire_to_shape(&item.wire), &plane) else {
            return Err(OcctError::NotDone {
                operation: "bullseye_face",
            });
        };
        let mut found = None;
        for index in (0..faces.len()).rev() {
            let state = ffi::b_rep_class::BRepClass_FaceClassifier_State(
                &faces[index].face,
                uv.X(),
                uv.Y(),
                tolerance,
            )
            .map_err(|e| occt_exception("bullseye_face", e))?;
            if state == TopAbs_State::TopAbs_IN {
                found = Some(index);
                break;
            }
            if state == TopAbs_State::TopAbs_OUT {
                let bound_state = ffi::b_rep_class::BRepClass_FaceClassifier_State(
                    &faces[index].bound,
                    uv.X(),
                    uv.Y(),
                    tolerance,
                )
                .map_err(|e| occt_exception("bullseye_face", e))?;
                if bound_state != TopAbs_State::TopAbs_IN {
                    continue;
                }
            }
        }
        if let Some(found) = found {
            let mut hole = ffi::topo_ds::TopoDS_Wire_to_owned(&item.wire);
            let direction = bullseye_wire_direction(&plane, &hole)?;
            if direction == 0 {
                return Err(OcctError::NotDone {
                    operation: "bullseye_face",
                });
            }
            if direction > 0 {
                hole = ffi::topo_ds::TopoDS_Wire_to_owned(TopoDS::Wire(&ffi::topo_ds::Reversed(
                    ffi::topo_ds::cast_wire_to_shape(&hole),
                )));
            }
            let mut face_shape = ffi::topo_ds::TopoDS_Shape_to_owned(
                ffi::topo_ds::cast_face_to_shape(&faces[found].face),
            );
            add_sub_shape(&builder_upcast, &mut face_shape, ffi::topo_ds::cast_wire_to_shape(&hole));
            faces[found].face = ffi::topo_ds::TopoDS_Face_to_owned(TopoDS::Face(&face_shape));
        } else {
            let mut outer = ffi::topo_ds::TopoDS_Wire_to_owned(&item.wire);
            let direction = bullseye_wire_direction(&plane, &outer)?;
            if direction == 0 {
                return Err(OcctError::NotDone {
                    operation: "bullseye_face",
                });
            }
            if direction < 0 {
                outer = ffi::topo_ds::TopoDS_Wire_to_owned(TopoDS::Wire(&ffi::topo_ds::Reversed(
                    ffi::topo_ds::cast_wire_to_shape(&outer),
                )));
            }
            let mut face = ffi::topo_ds::TopoDS_Face_new();
            ffi::b_rep::BRep_Builder_MakeFace(&builder, face.pin_mut(), &surface, confusion);
            let mut face_shape = ffi::topo_ds::TopoDS_Shape_to_owned(ffi::topo_ds::cast_face_to_shape(&face));
            add_sub_shape(&builder_upcast, &mut face_shape, ffi::topo_ds::cast_wire_to_shape(&outer));
            face = ffi::topo_ds::TopoDS_Face_to_owned(TopoDS::Face(&face_shape));
            let bound = ffi::b_rep_builder_api::BRepBuilderAPI_MakeFace_surface_wire(
                &surface,
                &outer,
                true,
            )
            .map_err(|e| occt_exception("bullseye_face", e))?;
            if !bound.IsDone() {
                return Err(OcctError::NotDone {
                    operation: "bullseye_face",
                });
            }
            faces.push(Driller {
                face,
                bound: ffi::topo_ds::TopoDS_Face_to_owned(bound.Face()),
            });
        }
    }

    if faces.is_empty() {
        return Err(OcctError::NotDone {
            operation: "bullseye_face",
        });
    }
    if faces.len() == 1 {
        return Ok(ffi::topo_ds::TopoDS_Shape_to_owned(
            ffi::topo_ds::cast_face_to_shape(&faces[0].face),
        ));
    }
    let mut result = ffi::topo_ds::TopoDS_Compound_new();
    builder_upcast.MakeCompound(result.pin_mut());
    let mut result_shape = ffi::topo_ds::TopoDS_Compound_as_shape(result);
    for driller in &faces {
        builder_upcast.Add(result_shape.pin_mut(), ffi::topo_ds::cast_face_to_shape(&driller.face));
    }
    Ok(result_shape)
}

/// The sub-shapes of `shape` whose type is `kind`, as a compound.
pub(crate) fn shape_sub_shapes_of_type(
    shape: &TopoDS_Shape,
    kind: TopAbs_ShapeEnum,
) -> UniquePtr<TopoDS_Shape> {
    if shape.IsNull() {
        return UniquePtr::null();
    }
    let mut map = ffi::top_tools::new_indexed_map_of_shape();
    ffi::top_exp::TopExp::MapShapes(shape, kind, map.pin_mut());
    let mut compound = ffi::topo_ds::TopoDS_Compound_new();
    let builder = ffi::b_rep::BRep_Builder_new();
    let builder = ffi::b_rep::BRep_Builder_upcast_to_topods_builder(&builder);
    builder.MakeCompound(compound.pin_mut());
    let mut compound_shape = ffi::topo_ds::TopoDS_Compound_as_shape(compound);
    for index in 1..=map.Extent() {
        builder.Add(compound_shape.pin_mut(), map.FindKey(index));
    }
    compound_shape
}

/// `Shape.makeOffsetShape`: a 3D offset with the fill-wall contract.
#[allow(clippy::too_many_arguments)]
pub(crate) fn offset_shape(
    shape: &TopoDS_Shape,
    offset: f64,
    tolerance: f64,
    inter: bool,
    self_inter: bool,
    offset_mode: i32,
    join: i32,
    fill: bool,
) -> Result<UniquePtr<TopoDS_Shape>, OcctError> {
    if shape.IsNull() {
        return Err(OcctError::NotDone {
            operation: "offset_shape",
        });
    }
    let mode = match offset_mode {
        1 => ffi::b_rep_offset_api::BRepOffset_Mode::BRepOffset_Pipe,
        2 => ffi::b_rep_offset_api::BRepOffset_Mode::BRepOffset_RectoVerso,
        _ => ffi::b_rep_offset_api::BRepOffset_Mode::BRepOffset_Skin,
    };
    let mut mk = ffi::b_rep_offset_api::BRepOffsetAPI_MakeOffsetShape_new();
    ffi::b_rep_offset_api::BRepOffsetAPI_MakeOffsetShape_PerformByJoin(
        mk.pin_mut(),
        shape,
        offset,
        tolerance,
        mode,
        inter,
        self_inter,
        join_type(join),
    )
    .map_err(|e| occt_exception("offset_shape", e))?;
    if !mk.IsDone() {
        return Err(OcctError::NotDone {
            operation: "offset_shape",
        });
    }
    let mut result = ffi::topo_ds::TopoDS_Shape_to_owned(mk.pin_mut().Shape());
    if result.IsNull() {
        return Err(OcctError::NotDone {
            operation: "offset_shape",
        });
    }

    if has_type(shape, TopAbs_ShapeEnum::TopAbs_SOLID) && !has_type(&result, TopAbs_ShapeEnum::TopAbs_SOLID) {
        let mut sewn = ffi::b_rep_builder_api::BRepBuilderAPI_Sewing_new_default();
        ffi::b_rep_builder_api::BRepBuilderAPI_Sewing_Add(sewn.pin_mut(), &result);
        ffi::b_rep_builder_api::BRepBuilderAPI_Sewing_Perform(sewn.pin_mut())
            .map_err(|e| occt_exception("offset_shape", e))?;
        let shell = ffi::topo_ds::TopoDS_Shape_to_owned(sewn.SewedShape());
        if shell.ShapeType() == TopAbs_ShapeEnum::TopAbs_SHELL {
            let mut solid = ffi::b_rep_builder_api::BRepBuilderAPI_MakeSolid_new(TopoDS::Shell(&shell));
            if solid.IsDone() {
                result = ffi::topo_ds::TopoDS_Shape_to_owned(
                    ffi::topo_ds::cast_solid_to_shape(solid.pin_mut().Solid()),
                );
            }
        }
    }

    if !fill {
        return Ok(result);
    }

    let mut free_check = ffi::shape_analysis::ShapeAnalysis_FreeBoundsProperties_new(shape)
        .map_err(|e| occt_exception("offset_shape", e))?;
    ffi::shape_analysis::ShapeAnalysis_FreeBoundsProperties_Perform(free_check.pin_mut())
        .map_err(|e| occt_exception("offset_shape", e))?;
    if ffi::shape_analysis::ShapeAnalysis_FreeBoundsProperties_NbClosedFreeBounds(&free_check) < 1 {
        return Err(OcctError::NotDone {
            operation: "offset_shape",
        });
    }

    let image = ffi::b_rep_offset_api::BRepOffsetAPI_MakeOffsetShape_OffsetEdgesFromShapes(&mk);
    let builder = ffi::b_rep::BRep_Builder_new();
    let builder_upcast = ffi::b_rep::BRep_Builder_upcast_to_topods_builder(&builder);
    let mut perimeter = ffi::topo_ds::TopoDS_Compound_new();
    builder_upcast.MakeCompound(perimeter.pin_mut());
    let mut perimeter_shape = ffi::topo_ds::TopoDS_Compound_as_shape(perimeter);

    for index in 1..=ffi::shape_analysis::ShapeAnalysis_FreeBoundsProperties_NbClosedFreeBounds(&free_check) {
        let original = ffi::shape_analysis::ShapeAnalysis_FreeBoundsProperties_ClosedFreeBound(
            &free_check,
            index,
        )
        .map_err(|e| occt_exception("offset_shape", e))?;
        let mut offset_wire = ffi::topo_ds::TopoDS_Wire_new();
        ffi::b_rep::BRep_Builder_MakeWire(&builder, offset_wire.pin_mut());
        let mut explorer = ffi::top_exp::TopExp_Explorer_new(
            ffi::topo_ds::cast_wire_to_shape(&original),
            TopAbs_ShapeEnum::TopAbs_EDGE,
        );
        while explorer.More() {
            if !image.HasImage(explorer.Current()) {
                return Err(OcctError::NotDone {
                    operation: "offset_shape",
                });
            }
            let mapped = image.Image(explorer.Current());
            let mut edge = None;
            let mut count = 0;
            let mut list = ffi::top_tools::TopTools_ListIteratorOfListOfShape_new(mapped);
            while list.More() {
                if list.Value().ShapeType() == TopAbs_ShapeEnum::TopAbs_EDGE {
                    count += 1;
                    edge = Some(ffi::topo_ds::TopoDS_Edge_to_owned(TopoDS::Edge(list.Value())));
                }
                list.pin_mut().Next();
            }
            if count != 1 {
                return Err(OcctError::NotDone {
                    operation: "offset_shape",
                });
            }
            let edge = edge.unwrap();
            let mut wire_shape = ffi::topo_ds::TopoDS_Shape_to_owned(
                ffi::topo_ds::cast_wire_to_shape(&offset_wire),
            );
            add_sub_shape(&builder_upcast, &mut wire_shape, ffi::topo_ds::cast_edge_to_shape(&edge));
            offset_wire = ffi::topo_ds::TopoDS_Wire_to_owned(TopoDS::Wire(&wire_shape));
            explorer.pin_mut().Next();
        }

        let mut generator = ffi::b_rep_offset_api::BRepOffsetAPI_ThruSections_new(false);
        generator.pin_mut().AddWire(&original);
        generator.pin_mut().AddWire(&offset_wire);
        generator.pin_mut().Build(&ffi::message::Message_ProgressRange_new());
        if !generator.IsDone() {
            return Err(OcctError::NotDone {
                operation: "offset_shape",
            });
        }
        builder_upcast.Add(perimeter_shape.pin_mut(), generator.pin_mut().Shape());
    }

    let mut sew = ffi::b_rep_builder_api::BRepBuilderAPI_Sewing_new_default();
    ffi::b_rep_builder_api::BRepBuilderAPI_Sewing_Add(sew.pin_mut(), shape);
    ffi::b_rep_builder_api::BRepBuilderAPI_Sewing_Add(sew.pin_mut(), &perimeter_shape);
    ffi::b_rep_builder_api::BRepBuilderAPI_Sewing_Add(sew.pin_mut(), &result);
    ffi::b_rep_builder_api::BRepBuilderAPI_Sewing_Perform(sew.pin_mut())
        .map_err(|e| occt_exception("offset_shape", e))?;
    let mut output = ffi::topo_ds::TopoDS_Shape_to_owned(sew.SewedShape());
    if output.ShapeType() == TopAbs_ShapeEnum::TopAbs_SHELL && output.Closed() {
        let mut solid = ffi::b_rep_builder_api::BRepBuilderAPI_MakeSolid_new(TopoDS::Shell(&output));
        if solid.IsDone() {
            let mut candidate = ffi::topo_ds::TopoDS_Solid_to_owned(solid.pin_mut().Solid());
            if ffi::b_rep_lib::BRepLib::OrientClosedSolid(candidate.pin_mut()) {
                output = ffi::topo_ds::TopoDS_Shape_to_owned(
                    ffi::topo_ds::cast_solid_to_shape(&candidate),
                );
            }
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::{is_closed, shape_tolerance};
    use cxx::UniquePtr;
    use opencascade_sys as ffi;
    use ffi::topo_ds::TopoDS_Shape;

    fn make_box(size: f64) -> UniquePtr<TopoDS_Shape> {
        let origin = ffi::gp::new_point(0.0, 0.0, 0.0);
        let mut make = ffi::b_rep_prim_api::BRepPrimAPI_MakeBox_new(&origin, size, size, size);
        let mut copier =
            ffi::b_rep_builder_api::BRepBuilderAPI_Copy_new(make.pin_mut().Shape(), true, false);
        assert!(copier.IsDone(), "a box copy");
        let shape = ffi::topo_ds::TopoDS_Shape_to_owned(copier.pin_mut().Shape());
        assert!(!shape.is_null(), "a box copy");
        shape
    }

    #[test]
    fn a_box_solid_is_closed_and_carries_a_tolerance() {
        let cube = make_box(10.0);
        assert!(is_closed(&cube), "a box is watertight");

        let tolerance = shape_tolerance(&cube);
        assert!(tolerance > 0.0, "tolerance was {tolerance}");
        assert!(tolerance < 1e-3, "tolerance was {tolerance}");
    }

    #[test]
    fn an_open_face_is_not_closed() {
        let cube = make_box(10.0);
        let faces = crate::shape::explore(
            cube.as_ref().unwrap(),
            ffi::top_abs::TopAbs_ShapeEnum::TopAbs_FACE,
            ffi::top_abs::TopAbs_ShapeEnum::TopAbs_SHAPE,
        );
        let face = faces.FindKey(1);
        assert!(!is_closed(face), "a box face is an open surface");
        assert!(shape_tolerance(face) > 0.0);
    }
}
