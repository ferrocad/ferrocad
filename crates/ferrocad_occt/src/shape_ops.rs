//! The remaining Part shape operations that used to live in
//! `ferrocad_part_cxx` (`fc_offset`, `fc_thick_solid`, `fc_section`, `fc_slice`,
//! `fc_general_fuse`, `fc_split_wire_into_faces`, …).
//!
//! Each entry mirrors one OCCT operation; FreeCAD's behaviour (the property
//! regime, the offset face rebuild) stays here, over the sys accessors.

use cxx::UniquePtr;
use ferrocad_curves::Vector3;
use opencascade_sys as ffi;

use ffi::geom_abs::GeomAbs_JoinType;
use ffi::top_abs::{TopAbs_Orientation, TopAbs_ShapeEnum};
use ffi::top_tools::TopTools_ListOfShape;
use ffi::topo_ds::{TopoDS, TopoDS_Shape};

/// Upstream's `JoinType` enum (0 = arc, 1 = tangent, 2 = intersection).
fn join_type(join: i32) -> GeomAbs_JoinType {
    match join {
        1 => GeomAbs_JoinType::GeomAbs_Tangent,
        2 => GeomAbs_JoinType::GeomAbs_Intersection,
        _ => GeomAbs_JoinType::GeomAbs_Arc,
    }
}

fn owned(shape: &TopoDS_Shape) -> UniquePtr<TopoDS_Shape> {
    ffi::topo_ds::TopoDS_Shape_to_owned(shape)
}

fn copy_list(list: &TopTools_ListOfShape) -> UniquePtr<TopTools_ListOfShape> {
    let mut out = ffi::top_tools::new_list_of_shape();
    let mut iterator = ffi::top_tools::TopTools_ListIteratorOfListOfShape_new(list);
    while iterator.More() {
        let _ = out.pin_mut().Append(iterator.Value());
        iterator.pin_mut().Next();
    }
    out
}

/// Planar offset of a face or wire (`BRepOffsetAPI_MakeOffset`).
pub(crate) fn offset(
    shape: &TopoDS_Shape,
    distance: f64,
    join: i32,
) -> Option<UniquePtr<TopoDS_Shape>> {
    if shape.IsNull() || !distance.is_finite() {
        return None;
    }
    let shape_type = shape.ShapeType();
    if shape_type != TopAbs_ShapeEnum::TopAbs_FACE && shape_type != TopAbs_ShapeEnum::TopAbs_WIRE {
        return None;
    }
    let join = join_type(join);
    let mut op = if shape_type == TopAbs_ShapeEnum::TopAbs_FACE {
        ffi::b_rep_offset_api::BRepOffsetAPI_MakeOffset_face_new(TopoDS::Face(shape), join)
    } else {
        ffi::b_rep_offset_api::BRepOffsetAPI_MakeOffset_wire_new(TopoDS::Wire(shape), join)
    };
    op.pin_mut().Perform(distance, 0.0);
    if !op.IsDone() {
        return None;
    }
    if shape_type != TopAbs_ShapeEnum::TopAbs_FACE {
        return Some(owned(op.pin_mut().Shape()));
    }

    // The face constructor returns the offset boundary; rebuild the face from
    // those wires, taking the first as the outer boundary and the rest as holes.
    let mut wires = Vec::new();
    let mut explorer = ffi::top_exp::TopExp_Explorer_new(
        op.pin_mut().Shape(),
        TopAbs_ShapeEnum::TopAbs_WIRE,
    );
    while explorer.More() {
        wires.push(ffi::topo_ds::TopoDS_Wire_to_owned(TopoDS::Wire(
            explorer.Current(),
        )));
        explorer.pin_mut().Next();
    }
    if wires.is_empty() {
        return None;
    }
    let first = wires.remove(0);
    let mut make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeFace_wire(&first, false);
    if !make.IsDone() {
        return None;
    }
    for wire in &wires {
        let reversed = ffi::topo_ds::Reversed(ffi::topo_ds::cast_wire_to_shape(wire));
        make.pin_mut()
            .add_wire(TopoDS::Wire(reversed.as_ref().unwrap()));
    }
    if !make.IsDone() {
        return None;
    }
    Some(ffi::topo_ds::TopoDS_Shape_to_owned(
        ffi::topo_ds::cast_face_to_shape(make.Face()),
    ))
}

/// Hollow a solid by `offset`, leaving `closing_faces` open.
pub(crate) fn thick_solid(
    shape: &TopoDS_Shape,
    closing_faces: &TopTools_ListOfShape,
    offset: f64,
    tolerance: f64,
) -> Option<UniquePtr<TopoDS_Shape>> {
    if shape.IsNull() || !offset.is_finite() {
        return None;
    }
    let mut op = ffi::b_rep_offset_api::BRepOffsetAPI_MakeThickSolid_new();
    let progress = ffi::message::Message_ProgressRange_new();
    op.pin_mut().MakeThickSolidByJoin(
        shape,
        closing_faces,
        offset,
        tolerance,
        ffi::b_rep_offset_api::BRepOffset_Mode::BRepOffset_Skin,
        false,
        false,
        GeomAbs_JoinType::GeomAbs_Arc,
        false,
        &progress,
    );
    if !op.IsDone() {
        return None;
    }
    Some(owned(op.pin_mut().Shape()))
}

/// The intersection of two shapes (`BRepAlgoAPI_Section`).
pub(crate) fn section(
    first: &TopoDS_Shape,
    second: &TopoDS_Shape,
) -> Option<UniquePtr<TopoDS_Shape>> {
    if first.IsNull() || second.IsNull() {
        return None;
    }
    let mut op = ffi::b_rep_algo_api::BRepAlgoAPI_Section_new(first, second);
    let progress = ffi::message::Message_ProgressRange_new();
    op.pin_mut().Build(&progress);
    if !op.IsDone() {
        return None;
    }
    Some(owned(op.pin_mut().Shape()))
}

/// The wires where `shape` meets the plane `dx*x + dy*y + dz*z = distance`.
pub(crate) fn slice(
    shape: &TopoDS_Shape,
    dx: f64,
    dy: f64,
    dz: f64,
    distance: f64,
) -> Option<UniquePtr<TopoDS_Shape>> {
    if shape.IsNull() || !distance.is_finite() {
        return None;
    }
    let magnitude = (dx * dx + dy * dy + dz * dz).sqrt();
    if !(magnitude > ffi::precision::Precision::Confusion()) || !magnitude.is_finite() {
        return None;
    }
    let plane = ffi::gp::gp_Pln_new_abcd(dx, dy, dz, -distance).ok()?;
    let mut op = ffi::b_rep_algo_api::BRepAlgoAPI_Section_new_plane(shape, &plane);
    let progress = ffi::message::Message_ProgressRange_new();
    op.pin_mut().Build(&progress);
    if !op.IsDone() {
        return None;
    }

    // Section returns loose edges; chain them into wires.
    let mut edges: Vec<_> = Vec::new();
    let mut explorer = ffi::top_exp::TopExp_Explorer_new(
        op.pin_mut().Shape(),
        TopAbs_ShapeEnum::TopAbs_EDGE,
    );
    while explorer.More() {
        edges.push(ffi::topo_ds::TopoDS_Edge_to_owned(TopoDS::Edge(
            explorer.Current(),
        )));
        explorer.pin_mut().Next();
    }

    let mut wires = Vec::new();
    while !edges.is_empty() {
        let mut make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeWire_new();
        make.pin_mut().add_edge(&edges.remove(0));
        let mut found = true;
        while found {
            found = false;
            let mut position = None;
            for (index, edge) in edges.iter().enumerate() {
                make.pin_mut().add_edge(edge);
                if make.Error()
                    != ffi::b_rep_builder_api::BRepBuilderAPI_WireError::BRepBuilderAPI_DisconnectedWire
                {
                    position = Some(index);
                    found = true;
                    break;
                }
            }
            if let Some(index) = position {
                edges.remove(index);
            }
        }
        if make.IsDone() {
            wires.push(ffi::topo_ds::TopoDS_Wire_to_owned(make.pin_mut().Wire()));
        }
    }
    if wires.is_empty() {
        return None;
    }
    let mut compound = ffi::topo_ds::TopoDS_Compound_new();
    let builder = ffi::b_rep::BRep_Builder_new();
    let builder = ffi::b_rep::BRep_Builder_upcast_to_topods_builder(&builder);
    builder.MakeCompound(compound.pin_mut());
    let mut compound_shape = ffi::topo_ds::TopoDS_Compound_as_shape(compound);
    for wire in &wires {
        builder.Add(compound_shape.pin_mut(), ffi::topo_ds::cast_wire_to_shape(wire));
    }
    Some(compound_shape)
}

/// All split parts of the arguments (General Fuse).
pub(crate) fn general_fuse(shapes: &TopTools_ListOfShape) -> Option<UniquePtr<TopoDS_Shape>> {
    if shapes.IsEmpty() {
        return None;
    }
    let mut builder = ffi::b_rep_algo_api::BRepAlgoAPI_BuilderAlgo_new();
    builder.pin_mut().SetArguments(shapes);
    builder.pin_mut().SetNonDestructive(true);
    if ffi::b_rep_algo_api::BRepAlgoAPI_BuilderAlgo_Build(builder.pin_mut()).is_err() {
        return None;
    }
    if !builder.IsDone() {
        return None;
    }
    Some(owned(builder.pin_mut().Shape()))
}

/// The split parts of input `index` from a General Fuse, as a compound.
pub(crate) fn general_fuse_modified(
    shapes: &TopTools_ListOfShape,
    index: i32,
    tolerance: f64,
) -> Option<UniquePtr<TopoDS_Shape>> {
    if shapes.IsEmpty() || index < 1 {
        return None;
    }
    let mut input = None;
    let mut position = 1;
    let mut iterator = ffi::top_tools::TopTools_ListIteratorOfListOfShape_new(shapes);
    while iterator.More() {
        if position == index {
            input = Some(owned(iterator.Value()));
            break;
        }
        iterator.pin_mut().Next();
        position += 1;
    }
    let input = input?;

    let mut builder = ffi::b_rep_algo_api::BRepAlgoAPI_BuilderAlgo_new();
    builder.pin_mut().SetArguments(shapes);
    builder.pin_mut().SetNonDestructive(true);
    if tolerance > 0.0 {
        ffi::b_rep_algo_api::BRepAlgoAPI_BuilderAlgo_SetFuzzyValue(builder.pin_mut(), tolerance);
    }
    if ffi::b_rep_algo_api::BRepAlgoAPI_BuilderAlgo_Build(builder.pin_mut()).is_err() {
        return None;
    }
    if !builder.IsDone() {
        return None;
    }

    let modified = builder.pin_mut().Modified(&input);
    if modified.IsEmpty() {
        return None;
    }
    let mut compound = ffi::topo_ds::TopoDS_Compound_new();
    let builder = ffi::b_rep::BRep_Builder_new();
    let builder = ffi::b_rep::BRep_Builder_upcast_to_topods_builder(&builder);
    builder.MakeCompound(compound.pin_mut());
    let mut compound_shape = ffi::topo_ds::TopoDS_Compound_as_shape(compound);
    let mut iterator = ffi::top_tools::TopTools_ListIteratorOfListOfShape_new(modified);
    while iterator.More() {
        builder.Add(compound_shape.pin_mut(), iterator.Value());
        iterator.pin_mut().Next();
    }
    Some(compound_shape)
}

/// A comp-solid of the given solids.
pub(crate) fn comp_solid(solids: &TopTools_ListOfShape) -> Option<UniquePtr<TopoDS_Shape>> {
    let mut compsolid = ffi::topo_ds::TopoDS_CompSolid_new();
    let builder = ffi::b_rep::BRep_Builder_new();
    let builder = ffi::b_rep::BRep_Builder_upcast_to_topods_builder(&builder);
    builder.MakeCompSolid(compsolid.pin_mut());
    let mut shape = ffi::topo_ds::TopoDS_CompSolid_as_shape(compsolid);
    let mut iterator = ffi::top_tools::TopTools_ListIteratorOfListOfShape_new(solids);
    while iterator.More() {
        builder.Add(shape.pin_mut(), iterator.Value());
        iterator.pin_mut().Next();
    }
    Some(shape)
}

/// A filled face through the boundary wires (`BRepFill_Filling`).
pub(crate) fn fill_wires(wires: &TopTools_ListOfShape) -> Option<UniquePtr<TopoDS_Shape>> {
    if wires.IsEmpty() {
        return None;
    }
    let mut filler = ffi::b_rep_fill::BRepFill_Filling_new();
    let mut iterator = ffi::top_tools::TopTools_ListIteratorOfListOfShape_new(wires);
    while iterator.More() {
        let wire = iterator.Value();
        if wire.IsNull() || wire.ShapeType() != TopAbs_ShapeEnum::TopAbs_WIRE {
            return None;
        }
        let mut edges = ffi::top_exp::TopExp_Explorer_new(wire, TopAbs_ShapeEnum::TopAbs_EDGE);
        while edges.More() {
            filler.pin_mut().Add(
                TopoDS::Edge(edges.Current()),
                ffi::geom_api::GeomAbs_Shape::GeomAbs_C0,
                true,
            );
            edges.pin_mut().Next();
        }
        iterator.pin_mut().Next();
    }
    if ffi::b_rep_fill::BRepFill_Filling_Build(filler.pin_mut()).is_err() {
        return None;
    }
    if !filler.IsDone() {
        return None;
    }
    let face = ffi::b_rep_fill::BRepFill_Filling_Face(&filler);
    if ffi::topo_ds::cast_face_to_shape(&face).IsNull() {
        return None;
    }
    Some(ffi::topo_ds::TopoDS_Shape_to_owned(
        ffi::topo_ds::cast_face_to_shape(&face),
    ))
}

/// A ruled surface spanning two edges (`BRepFill::Face`).
pub(crate) fn ruled_surface(
    first: &TopoDS_Shape,
    second: &TopoDS_Shape,
) -> Option<UniquePtr<TopoDS_Shape>> {
    if first.IsNull()
        || second.IsNull()
        || first.ShapeType() != TopAbs_ShapeEnum::TopAbs_EDGE
        || second.ShapeType() != TopAbs_ShapeEnum::TopAbs_EDGE
    {
        return None;
    }
    let face = ffi::b_rep_fill::BRepFill_Face(TopoDS::Edge(first), TopoDS::Edge(second)).ok()?;
    if ffi::topo_ds::cast_face_to_shape(&face).IsNull() {
        return None;
    }
    Some(ffi::topo_ds::TopoDS_Shape_to_owned(
        ffi::topo_ds::cast_face_to_shape(&face),
    ))
}

/// Sweep `profile` along `spine` (`BRepOffsetAPI_MakePipe`).
pub(crate) fn pipe(
    spine: &TopoDS_Shape,
    profile: &TopoDS_Shape,
    _tolerance: f64,
) -> Option<UniquePtr<TopoDS_Shape>> {
    if spine.IsNull() || profile.IsNull() {
        return None;
    }
    let wire = match spine.ShapeType() {
        TopAbs_ShapeEnum::TopAbs_WIRE => ffi::topo_ds::TopoDS_Wire_to_owned(TopoDS::Wire(spine)),
        TopAbs_ShapeEnum::TopAbs_EDGE => {
            let mut make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeWire_new();
            make.pin_mut().add_edge(TopoDS::Edge(spine));
            if !make.IsDone() {
                return None;
            }
            ffi::topo_ds::TopoDS_Wire_to_owned(make.pin_mut().Wire())
        }
        _ => return None,
    };
    let mut pipe = ffi::b_rep_offset_api::BRepOffsetAPI_MakePipe_new(&wire, profile);
    if !pipe.IsDone() {
        return None;
    }
    Some(owned(pipe.pin_mut().Shape()))
}

/// Remove sub-shapes (`BRepTools_ReShape`).
pub(crate) fn reshape_remove(
    shape: &TopoDS_Shape,
    removals: &TopTools_ListOfShape,
) -> Option<UniquePtr<TopoDS_Shape>> {
    if shape.IsNull() {
        return None;
    }
    let mut reshape = ffi::b_rep_tools::BRepTools_ReShape_new();
    let mut iterator = ffi::top_tools::TopTools_ListIteratorOfListOfShape_new(removals);
    while iterator.More() {
        reshape.pin_mut().Remove(iterator.Value());
        iterator.pin_mut().Next();
    }
    let result = ffi::b_rep_tools::BRepTools_ReShape_Apply(
        reshape.pin_mut(),
        shape,
        TopAbs_ShapeEnum::TopAbs_SHAPE,
    );
    if result.IsNull() {
        return None;
    }
    Some(result)
}

/// Replace sub-shapes (`BRepTools_ReShape`).
pub(crate) fn reshape_replace(
    shape: &TopoDS_Shape,
    from: &TopTools_ListOfShape,
    to: &TopTools_ListOfShape,
) -> Option<UniquePtr<TopoDS_Shape>> {
    if shape.IsNull() || from.Extent() != to.Extent() {
        return None;
    }
    let mut reshape = ffi::b_rep_tools::BRepTools_ReShape_new();
    let mut old = ffi::top_tools::TopTools_ListIteratorOfListOfShape_new(from);
    let mut new = ffi::top_tools::TopTools_ListIteratorOfListOfShape_new(to);
    while old.More() {
        reshape.pin_mut().Replace(old.Value(), new.Value());
        old.pin_mut().Next();
        new.pin_mut().Next();
    }
    let result = ffi::b_rep_tools::BRepTools_ReShape_Apply(
        reshape.pin_mut(),
        shape,
        TopAbs_ShapeEnum::TopAbs_SHAPE,
    );
    if result.IsNull() {
        return None;
    }
    Some(result)
}

/// `Shape.isValid`: every sub-shape passes `BRepCheck_Analyzer`.
pub(crate) fn is_valid(shape: &TopoDS_Shape) -> bool {
    if shape.IsNull() {
        return false;
    }
    let analyser = ffi::b_rep_check::BRepCheck_Analyzer_new(shape, true);
    analyser.IsValid()
}

/// `BRepBndLib::AddOptimal` with the protective gap dropped.
pub(crate) fn optimal_box(
    shape: &TopoDS_Shape,
    use_triangulation: bool,
    use_tolerance: bool,
) -> Option<(Vector3, Vector3)> {
    if shape.IsNull() {
        return None;
    }
    let mut box_ = ffi::bnd::Bnd_Box_new();
    ffi::b_rep_bnd_lib::BRepBndLib::AddOptimal(
        shape,
        box_.pin_mut(),
        use_triangulation,
        use_tolerance,
    );
    if box_.IsVoid() {
        return None;
    }
    box_.pin_mut().SetGap(0.0);
    let (mut xmin, mut ymin, mut zmin, mut xmax, mut ymax, mut zmax) =
        (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    box_.Get(&mut xmin, &mut ymin, &mut zmin, &mut xmax, &mut ymax, &mut zmax);
    Some((
        Vector3::new(xmin, ymin, zmin),
        Vector3::new(xmax, ymax, zmax),
    ))
}

/// The 3x3 inertia matrix (row-major), from `BRepGProp::VolumeProperties`.
pub(crate) fn inertia_matrix(shape: &TopoDS_Shape) -> Option<[f64; 9]> {
    if shape.IsNull() {
        return None;
    }
    let mut props = ffi::g_prop::GProps_new();
    ffi::b_rep_g_prop::BRepGProp::VolumeProperties(shape, props.pin_mut(), false, false, false);
    let matrix = ffi::g_prop::GProp_GProps_MatrixOfInertia(&props);
    let mut out = [0.0; 9];
    for row in 1..=3 {
        for col in 1..=3 {
            out[(row - 1) * 3 + (col - 1)] =
                ffi::gp::gp_Mat_Value(&matrix, row as i32, col as i32);
        }
    }
    Some(out)
}

/// The property regime upstream's `Part::getShapeProperties` uses: measure what
/// the shape contains.
fn measure_properties(shape: &TopoDS_Shape) -> Option<UniquePtr<ffi::g_prop::GProp_GProps>> {
    for kind in [
        TopAbs_ShapeEnum::TopAbs_SOLID,
        TopAbs_ShapeEnum::TopAbs_FACE,
        TopAbs_ShapeEnum::TopAbs_EDGE,
    ] {
        let mut explorer = ffi::top_exp::TopExp_Explorer_new(shape, kind);
        if !explorer.More() {
            continue;
        }
        let mut props = ffi::g_prop::GProps_new();
        match kind {
            TopAbs_ShapeEnum::TopAbs_SOLID => ffi::b_rep_g_prop::BRepGProp::VolumeProperties(
                shape,
                props.pin_mut(),
                false,
                false,
                false,
            ),
            TopAbs_ShapeEnum::TopAbs_FACE => ffi::b_rep_g_prop::BRepGProp::SurfaceProperties(
                shape,
                props.pin_mut(),
                false,
                false,
            ),
            _ => ffi::b_rep_g_prop::BRepGProp::LinearProperties(shape, props.pin_mut(), false, false),
        }
        return Some(props);
    }
    let mut vertices = ffi::top_exp::TopExp_Explorer_new(shape, TopAbs_ShapeEnum::TopAbs_VERTEX);
    if !vertices.More() {
        return None;
    }
    let (mut sx, mut sy, mut sz) = (0.0, 0.0, 0.0);
    let mut count = 0;
    while vertices.More() {
        let pnt = ffi::b_rep::BRep_Tool_Pnt(TopoDS::Vertex(vertices.Current()));
        sx += pnt.X();
        sy += pnt.Y();
        sz += pnt.Z();
        count += 1;
        vertices.pin_mut().Next();
    }
    let centre = ffi::gp::new_point(sx / count as f64, sy / count as f64, sz / count as f64);
    Some(ffi::g_prop::GProp_GProps_new_point(&centre))
}

/// `Shape.CenterOfMass` under the `getShapeProperties` rule.
pub(crate) fn center_of_mass(shape: &TopoDS_Shape) -> Option<Vector3> {
    if shape.IsNull() {
        return None;
    }
    let props = measure_properties(shape)?;
    let centre = ffi::g_prop::GProp_GProps_CentreOfMass(&props);
    Some(Vector3::new(centre.X(), centre.Y(), centre.Z()))
}

/// `Shape.PrincipalProperties`.
pub(crate) struct PrincipalValues {
    pub symmetry_axis: bool,
    pub symmetry_point: bool,
    pub moments: [f64; 3],
    pub axes: [Vector3; 3],
    pub radius_of_gyration: [f64; 3],
}

pub(crate) fn principal_properties(shape: &TopoDS_Shape) -> Option<PrincipalValues> {
    if shape.IsNull() {
        return None;
    }
    let props = measure_properties(shape)?;
    let principal = ffi::g_prop::GProp_GProps_PrincipalProperties(&props);
    let (mut lx, mut ly, mut lz) = (0.0, 0.0, 0.0);
    principal.Moments(&mut lx, &mut ly, &mut lz);
    let (mut rxx, mut ryy, mut rzz) = (0.0, 0.0, 0.0);
    principal.RadiusOfGyration(&mut rxx, &mut ryy, &mut rzz);
    Some(PrincipalValues {
        symmetry_axis: principal.HasSymmetryAxis(),
        symmetry_point: principal.HasSymmetryPoint(),
        moments: [lx, ly, lz],
        axes: [
            vec_of(principal.FirstAxisOfInertia()),
            vec_of(principal.SecondAxisOfInertia()),
            vec_of(principal.ThirdAxisOfInertia()),
        ],
        radius_of_gyration: [rxx, ryy, rzz],
    })
}

fn vec_of(vec: &ffi::gp::gp_Vec) -> Vector3 {
    Vector3::new(vec.X(), vec.Y(), vec.Z())
}

/// `Edge.split`: cut `edge` at the given parameters into a wire of pieces.
pub(crate) fn split_edge(
    shape: &TopoDS_Shape,
    parameters: &[f64],
) -> Option<UniquePtr<TopoDS_Shape>> {
    if shape.IsNull() || shape.ShapeType() != TopAbs_ShapeEnum::TopAbs_EDGE {
        return None;
    }
    let edge = TopoDS::Edge(shape);
    let adapt = ffi::b_rep_adaptor::BRepAdaptor_Curve_new(edge);
    let first = adapt.FirstParameter();
    let last = adapt.LastParameter();

    let mut params = Vec::with_capacity(parameters.len() + 2);
    params.push(first);
    for value in parameters {
        if *value <= first || *value >= last {
            return None;
        }
        params.push(*value);
    }
    params.push(last);
    params.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let geometric = ffi::b_rep_adaptor::BRepAdaptor_Curve_Curve(&adapt);
    let curve = ffi::geom_adaptor::GeomAdaptor_Curve_Curve(geometric);
    if curve.IsNull() {
        return None;
    }

    let mut make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeWire_new();
    let builder = ffi::b_rep::BRep_Builder_new();
    for i in 0..params.len() - 1 {
        let mut piece = ffi::b_rep_builder_api::BRepBuilderAPI_MakeEdge_CurveRange(
            &curve,
            params[i],
            params[i + 1],
        )
        .ok()?;
        if !piece.IsDone() {
            return None;
        }
        let mut piece_edge = ffi::topo_ds::TopoDS_Edge_to_owned(piece.pin_mut().Edge());
        ffi::b_rep::BRep_Builder_Transfert(&builder, edge, &piece_edge);
        ffi::b_rep::BRep_Builder_Range(&builder, &piece_edge, params[i], params[i + 1]);
        make.pin_mut().add_edge(&piece_edge);
    }
    if !make.IsDone() {
        return None;
    }
    Some(ffi::topo_ds::TopoDS_Shape_to_owned(
        ffi::topo_ds::cast_wire_to_shape(make.pin_mut().Wire()),
    ))
}

/// `Part.makeSplitShape`: `BRepFeat_SplitShape` over `shape`.
pub(crate) fn split_shape(
    shape: &TopoDS_Shape,
    tools: &TopTools_ListOfShape,
    faces: &TopTools_ListOfShape,
    _check_interior: bool,
    mut direct: std::pin::Pin<&mut TopTools_ListOfShape>,
    mut left: std::pin::Pin<&mut TopTools_ListOfShape>,
) -> bool {
    if shape.IsNull() || tools.Extent() != faces.Extent() {
        return false;
    }
    let mut split = ffi::b_rep_feat::BRepFeat_SplitShape_new_from_shape(shape);

    let mut tool = ffi::top_tools::TopTools_ListIteratorOfListOfShape_new(tools);
    let mut face = ffi::top_tools::TopTools_ListIteratorOfListOfShape_new(faces);
    while tool.More() {
        let cutter = tool.Value();
        let target = face.Value();
        if cutter.IsNull()
            || target.IsNull()
            || target.ShapeType() != TopAbs_ShapeEnum::TopAbs_FACE
        {
            return false;
        }
        match cutter.ShapeType() {
            TopAbs_ShapeEnum::TopAbs_EDGE => {
                if ffi::b_rep_feat::BRepFeat_SplitShape_Add_edge_face(
                    split.pin_mut(),
                    TopoDS::Edge(cutter),
                    TopoDS::Face(target),
                )
                .is_err()
                {
                    return false;
                }
            }
            TopAbs_ShapeEnum::TopAbs_WIRE => {
                if ffi::b_rep_feat::BRepFeat_SplitShape_Add_wire_face(
                    split.pin_mut(),
                    TopoDS::Wire(cutter),
                    TopoDS::Face(target),
                )
                .is_err()
                {
                    return false;
                }
            }
            _ => return false,
        }
        tool.pin_mut().Next();
        face.pin_mut().Next();
    }

    if ffi::b_rep_feat::BRepFeat_SplitShape_Build(split.pin_mut()).is_err() {
        return false;
    }

    ffi::top_tools::list_clear(direct.as_mut());
    ffi::top_tools::list_clear(left.as_mut());
    let mut it = ffi::top_tools::TopTools_ListIteratorOfListOfShape_new(ffi::b_rep_feat::BRepFeat_SplitShape_DirectLeft(split.pin_mut()));
    while it.More() {
        direct.as_mut().Append(it.Value());
        it.pin_mut().Next();
    }
    let mut it = ffi::top_tools::TopTools_ListIteratorOfListOfShape_new(ffi::b_rep_feat::BRepFeat_SplitShape_Left(split.pin_mut()));
    while it.More() {
        left.as_mut().Append(it.Value());
        it.pin_mut().Next();
    }
    true
}

/// Split one (possibly self-intersecting) wire into planar faces.
pub(crate) fn split_wire_into_faces(wire: &TopoDS_Shape) -> Option<UniquePtr<TopoDS_Shape>> {
    if wire.IsNull() || wire.ShapeType() != TopAbs_ShapeEnum::TopAbs_WIRE {
        return None;
    }
    let mut copier = ffi::b_rep_builder_api::BRepBuilderAPI_Copy_new(wire, true, false);
    let source = if copier.IsDone() {
        owned(copier.pin_mut().Shape())
    } else {
        owned(wire)
    };

    let mut edges = ffi::top_tools::new_list_of_shape();
    let mut explorer = ffi::top_exp::TopExp_Explorer_new(&source, TopAbs_ShapeEnum::TopAbs_EDGE);
    while explorer.More() {
        let _ = edges.pin_mut().Append(explorer.Current());
        explorer.pin_mut().Next();
    }
    if edges.IsEmpty() {
        return None;
    }

    let plane = find_plane(&edges)?;
    let self_split = split_self_intersecting(&edges, &plane);
    let split = split_at_intersections(&self_split);
    let faces = collect_bounded_faces(&split, &plane)?;

    let mut compound = ffi::topo_ds::TopoDS_Compound_new();
    let builder = ffi::b_rep::BRep_Builder_new();
    let builder = ffi::b_rep::BRep_Builder_upcast_to_topods_builder(&builder);
    builder.MakeCompound(compound.pin_mut());
    let mut compound_shape = ffi::topo_ds::TopoDS_Compound_as_shape(compound);
    for face in &faces {
        builder.Add(
            compound_shape.pin_mut(),
            ffi::topo_ds::cast_face_to_shape(face),
        );
    }
    Some(compound_shape)
}

/// The plane containing every edge, or `None`.
fn find_plane(edges: &TopTools_ListOfShape) -> Option<UniquePtr<ffi::gp::gp_Pln>> {
    if edges.IsEmpty() {
        return None;
    }
    let mut compound = ffi::topo_ds::TopoDS_Compound_new();
    let builder = ffi::b_rep::BRep_Builder_new();
    let builder = ffi::b_rep::BRep_Builder_upcast_to_topods_builder(&builder);
    builder.MakeCompound(compound.pin_mut());
    let mut compound_shape = ffi::topo_ds::TopoDS_Compound_as_shape(compound);
    let mut iterator = ffi::top_tools::TopTools_ListIteratorOfListOfShape_new(edges);
    while iterator.More() {
        let mut copier =
            ffi::b_rep_builder_api::BRepBuilderAPI_Copy_new(iterator.Value(), true, false);
        if copier.IsDone() {
            builder.Add(compound_shape.pin_mut(), copier.pin_mut().Shape());
        }
        iterator.pin_mut().Next();
    }
    let finder = ffi::b_rep_lib::BRepLib_FindSurface_new(&compound_shape, -1.0, true);
    if !finder.Found() {
        return None;
    }
    let surface = ffi::b_rep_lib::BRepLib_FindSurface_Surface(&finder);
    if surface.IsNull() {
        return None;
    }
    let adaptor = ffi::geom_adaptor::GeomAdaptor_Surface_new(&surface).ok()?;
    ffi::geom_adaptor::GeomAdaptor_Surface_Plane(&adaptor).ok()
}

/// Split each edge at the points where it crosses itself.
fn split_self_intersecting(
    edges: &TopTools_ListOfShape,
    plane: &ffi::gp::gp_Pln,
) -> UniquePtr<TopTools_ListOfShape> {
    let mut out = ffi::top_tools::new_list_of_shape();
    let tol = ffi::precision::Precision::Confusion();
    let mut iterator = ffi::top_tools::TopTools_ListIteratorOfListOfShape_new(edges);
    while iterator.More() {
        let edge = TopoDS::Edge(iterator.Value());
        let mut first = 0.0;
        let mut last = 0.0;
        let curve = ffi::b_rep::BRep_Tool_Curve(edge, &mut first, &mut last);
        let keep = |out: &mut UniquePtr<TopTools_ListOfShape>| {
            let _ = out.pin_mut().Append(iterator.Value());
        };
        if curve.IsNull()
            || ffi::geom::GeomCurve_IsKind_line(&curve)
            || ffi::geom::GeomCurve_IsKind_circle(&curve)
            || ffi::geom::GeomCurve_IsKind_ellipse(&curve)
            || ffi::geom::GeomCurve_IsKind_hyperbola(&curve)
            || ffi::geom::GeomCurve_IsKind_parabola(&curve)
        {
            keep(&mut out);
            iterator.pin_mut().Next();
            continue;
        }
        if ffi::geom::GeomCurve_IsKind_trimmed(&curve) {
            let trimmed = ffi::geom::GeomCurve_DownCast_trimmed(&curve);
            if !trimmed.IsNull() {
                let upcast = ffi::geom::new_HandleGeomCurve_from_HandleGeom_TrimmedCurve(&trimmed);
                first = ffi::geom::HandleGeomCurve_FirstParameter(&upcast);
                last = ffi::geom::HandleGeomCurve_LastParameter(&upcast);
            }
        }
        let Ok(curve2d) = ffi::geom_api::GeomAPI_To2d(&curve, plane) else {
            keep(&mut out);
            iterator.pin_mut().Next();
            continue;
        };
        if curve2d.IsNull() {
            keep(&mut out);
            iterator.pin_mut().Next();
            continue;
        }
        let mut params = Vec::new();
        let inter = ffi::geom_api::Geom2dAPI_InterCurveCurve_new(&curve2d, tol);
        if let Ok(inter) = inter {
            for i in 1..=inter.NbPoints() {
                let point = ffi::geom_api::Geom2dAPI_InterCurveCurve_Point(&inter, i);
                if let Ok(projection) = ffi::geom_api::Geom2dAPI_ProjectPointOnCurve_new(
                    &point, &curve2d, first, last,
                ) {
                    for j in 1..=projection.NbPoints() {
                        let param =
                            ffi::geom_api::Geom2dAPI_ProjectPointOnCurve_Parameter(&projection, j)
                                .ok()
                                .unwrap_or(0.0);
                        if param - first > tol && last - param > tol {
                            params.push(param);
                        }
                    }
                }
            }
        }
        if params.is_empty() {
            keep(&mut out);
            iterator.pin_mut().Next();
            continue;
        }
        params.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        params.dedup_by(|a, b| (*b - *a).abs() < tol);

        let mut previous = first;
        for param in &params {
            if *param - previous > tol {
                if let Ok(mut piece) =
                    ffi::b_rep_builder_api::BRepBuilderAPI_MakeEdge_CurveRange(&curve, previous, *param)
                {
                    if piece.IsDone() {
                        let _ = out
                            .pin_mut()
                            .Append(ffi::topo_ds::cast_edge_to_shape(piece.pin_mut().Edge()));
                    }
                }
                previous = *param;
            }
        }
        if last - previous > tol {
            if let Ok(mut piece) =
                ffi::b_rep_builder_api::BRepBuilderAPI_MakeEdge_CurveRange(&curve, previous, last)
            {
                if piece.IsDone() {
                    let _ = out
                        .pin_mut()
                        .Append(ffi::topo_ds::cast_edge_to_shape(piece.pin_mut().Edge()));
                }
            }
        }
        iterator.pin_mut().Next();
    }
    out
}

/// Split the edges against each other at their intersections.
fn split_at_intersections(edges: &TopTools_ListOfShape) -> UniquePtr<TopTools_ListOfShape> {
    if edges.Extent() <= 1 {
        return copy_list(edges);
    }
    let mut out = ffi::top_tools::new_list_of_shape();
    let mut splitter = ffi::b_rep_algo_api::BRepAlgoAPI_BuilderAlgo_new();
    splitter.pin_mut().SetArguments(edges);
    splitter.pin_mut().SetNonDestructive(true);
    if ffi::b_rep_algo_api::BRepAlgoAPI_BuilderAlgo_Build(splitter.pin_mut()).is_err() {
        return copy_list(edges);
    }
    if !splitter.IsDone() {
        return copy_list(edges);
    }
    let mut explorer = ffi::top_exp::TopExp_Explorer_new(
        splitter.pin_mut().Shape(),
        TopAbs_ShapeEnum::TopAbs_EDGE,
    );
    while explorer.More() {
        let _ = out.pin_mut().Append(explorer.Current());
        explorer.pin_mut().Next();
    }
    if out.IsEmpty() {
        return copy_list(edges);
    }
    out
}

/// Build the bounded regions the edges enclose inside `plane`.
fn collect_bounded_faces(
    edges: &TopTools_ListOfShape,
    plane: &ffi::gp::gp_Pln,
) -> Option<Vec<UniquePtr<ffi::topo_ds::TopoDS_Face>>> {
    if edges.IsEmpty() {
        return None;
    }
    let mut box_ = ffi::bnd::Bnd_Box_new();
    let mut iterator = ffi::top_tools::TopTools_ListIteratorOfListOfShape_new(edges);
    while iterator.More() {
        ffi::b_rep_bnd_lib::BRepBndLib::Add(iterator.Value(), box_.pin_mut(), false);
        iterator.pin_mut().Next();
    }
    let extent = f64::max(1.0e8, 10.0 * box_.SquareExtent().max(0.0).sqrt());
    let make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeFace_Plane(
        plane, -extent, extent, -extent, extent,
    )
    .ok()?;
    if !make.IsDone() {
        return None;
    }
    let base = ffi::topo_ds::TopoDS_Face_to_owned(make.Face());

    let mut face_edges = ffi::top_tools::new_list_of_shape();
    let mut iterator = ffi::top_tools::TopTools_ListIteratorOfListOfShape_new(edges);
    while iterator.More() {
        let edge = TopoDS::Edge(iterator.Value());
        let mut fwd = owned(ffi::topo_ds::cast_edge_to_shape(edge));
        fwd.pin_mut()
            .set_orientation(TopAbs_Orientation::TopAbs_FORWARD);
        let _ = face_edges.pin_mut().Append(&fwd);
        let mut rev = owned(ffi::topo_ds::cast_edge_to_shape(edge));
        rev.pin_mut()
            .set_orientation(TopAbs_Orientation::TopAbs_REVERSED);
        let _ = face_edges.pin_mut().Append(&rev);
        iterator.pin_mut().Next();
    }
    ffi::b_rep_lib::BRepLib_BuildPCurveForEdgesOnPlane(&face_edges, &base).ok()?;

    let mut builder = ffi::bop_algo::BOPAlgo_BuilderFace_new();
    builder.pin_mut().SetFace(&base);
    builder.pin_mut().SetShapes(&face_edges);
    ffi::bop_algo::BOPAlgo_BuilderFace_SetAvoidInternalShapes(builder.pin_mut(), true);
    if ffi::bop_algo::BOPAlgo_BuilderFace_Perform(builder.pin_mut()).is_err() {
        return None;
    }
    if builder.HasErrors() {
        return None;
    }

    let outer_threshold = extent * extent;
    let mut faces = Vec::new();
    let mut areas = ffi::top_tools::TopTools_ListIteratorOfListOfShape_new(builder.Areas());
    while areas.More() {
        let mut face_box = ffi::bnd::Bnd_Box_new();
        ffi::b_rep_bnd_lib::BRepBndLib::Add(areas.Value(), face_box.pin_mut(), false);
        if face_box.SquareExtent() > outer_threshold {
            areas.pin_mut().Next();
            continue;
        }
        let mut props = ffi::g_prop::GProps_new();
        ffi::b_rep_g_prop::BRepGProp::SurfaceProperties(areas.Value(), props.pin_mut(), false, false);
        if props.Mass() < ffi::precision::Precision::Confusion() {
            areas.pin_mut().Next();
            continue;
        }
        faces.push(ffi::topo_ds::TopoDS_Face_to_owned(TopoDS::Face(areas.Value())));
        areas.pin_mut().Next();
    }
    if faces.is_empty() {
        return None;
    }
    Some(faces)
}
