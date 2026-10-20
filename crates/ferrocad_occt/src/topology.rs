//! Tessellation, element enumeration, adjacency and fillet/chamfer lineage over OCCT.
//!
//! The neutral types live in [`ferrocad_geom`]; this module is the OCCT reading of
//! them. Two OCCT behaviours drive the implementation:
//!
//! - enumerate sub-shapes with `TopExp::MapShapes`, not `TopExp_Explorer` (the
//!   explorer visits a shared edge once per adjacent face);
//! - `TopExp::MapShapesAndAncestors` is ancestor-only: it maps a sub-shape to its
//!   containing elements, so the descendant direction needs an explorer.

use cxx::UniquePtr;
use ferrocad_geom::{
    Continuity, ElementKind, ElementRef, GeomError, History, OpResult, Shape, Tessellation,
    TessellationOptions,
};
use opencascade_sys as ffi;

use ffi::top_abs::{TopAbs_Orientation, TopAbs_ShapeEnum};
use ffi::top_tools::{TopTools_IndexedMapOfShape, TopTools_ListOfShape};
use ffi::topo_ds::{TopoDS, TopoDS_Shape};

use crate::bridge;
use crate::error::OcctError;
use crate::shape::{downcast, erase, new_map, sub_shape_map};

type Map = TopTools_IndexedMapOfShape;

/// The OCCT enumeration for a neutral element kind.
fn shape_enum(kind: ElementKind) -> TopAbs_ShapeEnum {
    match kind {
        ElementKind::Solid => TopAbs_ShapeEnum::TopAbs_SOLID,
        ElementKind::Shell => TopAbs_ShapeEnum::TopAbs_SHELL,
        ElementKind::Face => TopAbs_ShapeEnum::TopAbs_FACE,
        ElementKind::Wire => TopAbs_ShapeEnum::TopAbs_WIRE,
        ElementKind::Edge => TopAbs_ShapeEnum::TopAbs_EDGE,
        ElementKind::Vertex => TopAbs_ShapeEnum::TopAbs_VERTEX,
    }
}

/// The neutral kind of an OCCT sub-shape, when it has one.
fn kind_of(shape: &TopoDS_Shape) -> Option<ElementKind> {
    match shape.ShapeType() {
        TopAbs_ShapeEnum::TopAbs_SOLID => Some(ElementKind::Solid),
        TopAbs_ShapeEnum::TopAbs_SHELL => Some(ElementKind::Shell),
        TopAbs_ShapeEnum::TopAbs_FACE => Some(ElementKind::Face),
        TopAbs_ShapeEnum::TopAbs_WIRE => Some(ElementKind::Wire),
        TopAbs_ShapeEnum::TopAbs_EDGE => Some(ElementKind::Edge),
        TopAbs_ShapeEnum::TopAbs_VERTEX => Some(ElementKind::Vertex),
        _ => None,
    }
}

/// Coarsest to finest. Used to decide which way an adjacency query must go.
fn rank(kind: ElementKind) -> u8 {
    match kind {
        ElementKind::Solid => 0,
        ElementKind::Shell => 1,
        ElementKind::Face => 2,
        ElementKind::Wire => 3,
        ElementKind::Edge => 4,
        ElementKind::Vertex => 5,
    }
}

/// The `index`-th (1-based) element of `map`, named after its kind and position.
fn element_ref(map: &Map, index: i32, kind: ElementKind) -> ElementRef {
    ElementRef {
        shape: erase(map.FindKey(index)),
        name: format!("{}{index}", kind.prefix()),
    }
}

/// The index of `shape` in `map`, ignoring orientation, or `None`.
fn index_in(map: &Map, shape: &TopoDS_Shape) -> Option<i32> {
    let index = bridge::fc_indexed_map_find_index_same(map, shape);
    (index > 0).then_some(index)
}

/// The bounding-box diagonal, for a scale-derived tessellation deflection.
fn bounding_diagonal(shape: &TopoDS_Shape) -> f64 {
    let mut bbox = ffi::bnd::Bnd_Box_new();
    ffi::b_rep_bnd_lib::BRepBndLib::Add(shape, bbox.pin_mut(), false);
    if bbox.IsVoid() {
        return 0.0;
    }
    let (mut x_min, mut y_min, mut z_min) = (0.0, 0.0, 0.0);
    let (mut x_max, mut y_max, mut z_max) = (0.0, 0.0, 0.0);
    bbox.Get(
        &mut x_min, &mut y_min, &mut z_min, &mut x_max, &mut y_max, &mut z_max,
    );
    ((x_max - x_min).powi(2) + (y_max - y_min).powi(2) + (z_max - z_min).powi(2)).sqrt()
}

/// Tessellate `shape` into one mesh whose triangles, segments and vertices carry
/// their sub-element part id.
///
/// The mesh is flat-shaded (`normals` is empty): positions from faces, edges and
/// vertices share one array, so per-vertex normals would have to cover all three.
pub(crate) fn tessellate(
    shape: &Shape,
    options: &TessellationOptions,
) -> Result<Tessellation, GeomError> {
    let occt = downcast(shape)?;
    let guard = occt.borrow();
    let s = guard
        .as_ref()
        .ok_or(OcctError::InvalidInput { what: "null shape" })?;

    let diagonal = bounding_diagonal(s);
    let deflection = match options.linear_deflection {
        Some(d) if options.relative => d * diagonal.max(1e-9),
        Some(d) => d,
        None => (diagonal * 1e-3).max(1e-6),
    };
    let mesh = ffi::b_rep_mesh::IncrementalMesh_new(s, deflection);
    if !mesh.IsDone() {
        return Err(OcctError::NotDone {
            operation: "tessellate",
        }
        .into());
    }

    let faces = sub_shape_map(s, TopAbs_ShapeEnum::TopAbs_FACE);
    let edges = sub_shape_map(s, TopAbs_ShapeEnum::TopAbs_EDGE);
    let vertices = sub_shape_map(s, TopAbs_ShapeEnum::TopAbs_VERTEX);
    let face_count = faces.Extent() as u32;
    let edge_count = edges.Extent() as u32;

    let mut out = Tessellation::default();
    for index in 1..=face_count as i32 {
        out.parts.push(format!("Face{index}"));
    }
    for index in 1..=edge_count as i32 {
        out.parts.push(format!("Edge{index}"));
    }
    for index in 1..=vertices.Extent() {
        out.parts.push(format!("Vertex{index}"));
    }

    // Faces: each face's triangulation is flattened into the shared arrays, so the
    // per-face vertex duplication the picker needs is preserved.
    for face_index in 1..=faces.Extent() {
        let face_shape = faces.FindKey(face_index);
        let face = TopoDS::Face(face_shape);
        let mut location = ffi::top_loc::Location_new();
        let handle = ffi::b_rep::BRep_Tool_Triangulation(face, location.pin_mut());
        if handle.IsNull() {
            continue;
        }
        let triangulation = ffi::poly::Handle_Poly_Triangulation_Get(&handle).map_err(|_| {
            OcctError::InvalidInput {
                what: "null triangulation",
            }
        })?;
        let trsf = ffi::top_loc::TopLoc_Location_Transformation(&location);
        let reversed = matches!(
            face_shape.Orientation(),
            TopAbs_Orientation::TopAbs_REVERSED
        );

        let base = out.positions.len() as u32;
        for node in 1..=triangulation.NbNodes() {
            let mut point = ffi::poly::Poly_Triangulation_Node(triangulation, node);
            point.pin_mut().Transform(&trsf);
            out.positions
                .push([point.X() as f32, point.Y() as f32, point.Z() as f32]);
        }
        for triangle_index in 1..=triangulation.NbTriangles() {
            let triangle = triangulation.Triangle(triangle_index);
            let a = base + (triangle.Value(1) - 1) as u32;
            let b = base + (triangle.Value(2) - 1) as u32;
            let c = base + (triangle.Value(3) - 1) as u32;
            // OCCT stores the face orientation separately; flip the winding so the
            // renderer's flat normal points out of the solid.
            if reversed {
                out.triangles.extend([a, c, b]);
            } else {
                out.triangles.extend([a, b, c]);
            }
            out.triangle_parts.push((face_index - 1) as u32);
        }
    }

    // Edges: one polyline per topological edge, sampled from its curve rather than
    // from the face triangulations (which duplicate shared edges).
    for edge_index in 1..=edges.Extent() {
        let edge = TopoDS::Edge(edges.FindKey(edge_index));
        let curve = ffi::b_rep_adaptor::BRepAdaptor_Curve_new(edge);
        let sampler =
            ffi::gc_pnts::TangentialDeflection_new(&curve, options.angular_deflection, deflection);
        let points = sampler.NbPoints();
        if points < 2 {
            continue;
        }
        let base = out.positions.len() as u32;
        for point_index in 1..=points {
            let point = ffi::gc_pnts::GCPnts_TangentialDeflection_Value(&sampler, point_index);
            out.positions
                .push([point.X() as f32, point.Y() as f32, point.Z() as f32]);
        }
        for segment in 0..(points - 1) {
            out.edges.push(base + segment as u32);
            out.edges.push(base + segment as u32 + 1);
            out.segment_parts.push(face_count + (edge_index - 1) as u32);
        }
    }

    // Vertices: one position each, so a vertex pick has geometry.
    for vertex_index in 1..=vertices.Extent() {
        let vertex = TopoDS::Vertex(vertices.FindKey(vertex_index));
        let point = ffi::b_rep::BRep_Tool_Pnt(vertex);
        out.positions
            .push([point.X() as f32, point.Y() as f32, point.Z() as f32]);
        out.vertices.push((out.positions.len() - 1) as u32);
        out.vertex_parts
            .push(face_count + edge_count + (vertex_index - 1) as u32);
    }

    Ok(out)
}

/// Enumerate the sub-elements of `kind`, in the kernel's own order.
pub(crate) fn elements(shape: &Shape, kind: ElementKind) -> Vec<ElementRef> {
    let Ok(occt) = downcast(shape) else {
        return Vec::new();
    };
    let guard = occt.borrow();
    let Some(s) = guard.as_ref() else {
        return Vec::new();
    };
    let map = sub_shape_map(s, shape_enum(kind));
    (1..=map.Extent())
        .map(|index| element_ref(&map, index, kind))
        .collect()
}

/// The elements of `kind` related to `element`.
///
/// Coarser targets use `MapShapesAndAncestors` (which only walks upwards); finer
/// targets use an explorer over the element itself.
pub(crate) fn ancestors(shape: &Shape, element: &ElementRef, kind: ElementKind) -> Vec<ElementRef> {
    let Ok(occt) = downcast(shape) else {
        return Vec::new();
    };
    let guard = occt.borrow();
    let Some(s) = guard.as_ref() else {
        return Vec::new();
    };
    let Ok(element_occt) = downcast(&element.shape) else {
        return Vec::new();
    };
    let element_guard = element_occt.borrow();
    let Some(source_shape) = element_guard.as_ref() else {
        return Vec::new();
    };
    let Some(source_kind) = kind_of(source_shape) else {
        return Vec::new();
    };

    let target_map = sub_shape_map(s, shape_enum(kind));

    if rank(kind) >= rank(source_kind) {
        let sub = sub_shape_map(source_shape, shape_enum(kind));
        (1..=sub.Extent())
            .filter_map(|index| index_in(&target_map, sub.FindKey(index)))
            .map(|index| element_ref(&target_map, index, kind))
            .collect()
    } else {
        let mut data = ffi::top_tools::new_indexed_data_map_of_shape_list_of_shape();
        ffi::top_exp::TopExp::MapShapesAndAncestors(
            s,
            shape_enum(source_kind),
            shape_enum(kind),
            data.pin_mut(),
        );
        let index = bridge::fc_data_map_find_index_same(&data, source_shape);
        if index == 0 {
            return Vec::new();
        }
        let list: &TopTools_ListOfShape = data.FindFromIndex(index);
        let count = bridge::fc_list_size(list);
        (1..=count)
            .filter_map(|entry| {
                let ancestor = bridge::fc_list_value(list, entry);
                index_in(&target_map, ancestor).map(|index| element_ref(&target_map, index, kind))
            })
            .collect()
    }
}

/// The continuity between two adjacent edges, when they share a vertex.
///
/// OCCT answers this with the tangents at the shared vertex: equal (up to sign)
/// tangents are [`Continuity::G1`], anything else is [`Continuity::C0`]. Higher
/// orders (C1/G2/C2) are not distinguished; fillet propagation only needs G1.
pub(crate) fn continuity(_shape: &Shape, a: &ElementRef, b: &ElementRef) -> Option<Continuity> {
    let a_occt = downcast(&a.shape).ok()?;
    let b_occt = downcast(&b.shape).ok()?;
    let a_guard = a_occt.borrow();
    let b_guard = b_occt.borrow();
    let edge_a = a_guard.as_ref()?;
    let edge_b = b_guard.as_ref()?;
    if kind_of(edge_a) != Some(ElementKind::Edge) || kind_of(edge_b) != Some(ElementKind::Edge) {
        return None;
    }

    let mut tangents = Vec::new();
    bridge::fc_edge_tangents(TopoDS::Edge(edge_a), TopoDS::Edge(edge_b), &mut tangents);
    if tangents.len() != 6 {
        return None;
    }
    let dot = tangents[0] * tangents[3] + tangents[1] * tangents[4] + tangents[2] * tangents[5];
    // `abs`: the two edges may be oriented towards or away from the shared vertex.
    Some(if dot.abs() >= 0.999_999 {
        Continuity::G1
    } else {
        Continuity::C0
    })
}

/// A `FaceN` reference into `map`.
fn face_ref(map: &Map, index: i32) -> ElementRef {
    ElementRef {
        shape: erase(map.FindKey(index)),
        name: format!("Face{index}"),
    }
}

/// Build the lineage of one input edge from an operation's history.
fn edge_lineage<F>(
    input: &ElementRef,
    input_shape: &TopoDS_Shape,
    out_faces: &Map,
    mut probe: F,
) -> History
where
    F: FnMut(&TopoDS_Shape, &mut UniquePtr<Map>, &mut UniquePtr<Map>) -> bool,
{
    let mut modified = new_map();
    let mut generated = new_map();
    let deleted = probe(input_shape, &mut modified, &mut generated);

    let mut history = History::default();
    for entry in 1..=modified.Extent() {
        let out = modified.FindKey(entry);
        let index = bridge::fc_indexed_map_find_index(out_faces, out);
        if index > 0 {
            history
                .modified
                .push((input.clone(), face_ref(out_faces, index)));
        }
    }
    for entry in 1..=generated.Extent() {
        let out = generated.FindKey(entry);
        let index = bridge::fc_indexed_map_find_index(out_faces, out);
        if index > 0 {
            history
                .generated
                .push((input.clone(), face_ref(out_faces, index)));
        }
    }
    // An input can be both consumed and the ancestor of a generated element: a
    // filleted edge is deleted while the fillet face is generated from it. Record
    // both; the element map retires the name and the generated pair says where the
    // new face came from.
    if deleted {
        history.deleted.push(input.clone());
    }
    history
}

fn merge_into(into: &mut History, from: History) {
    into.generated.extend(from.generated);
    into.modified.extend(from.modified);
    into.deleted.extend(from.deleted);
}

/// Round `edges` and report the generated faces.
pub(crate) fn fillet(
    shape: &Shape,
    edges: &[ElementRef],
    radius: f64,
) -> Result<OpResult, GeomError> {
    if edges.is_empty() {
        return Err(OcctError::InvalidInput {
            what: "fillet needs at least one edge",
        }
        .into());
    }
    let occt = downcast(shape)?;
    let guard = occt.borrow();
    let s = guard
        .as_ref()
        .ok_or(OcctError::InvalidInput { what: "null shape" })?;

    let mut op = ffi::b_rep_fillet_api::BRepFilletAPI_MakeFillet_new(s);
    for edge in edges {
        let edge_occt = downcast(&edge.shape)?;
        let edge_guard = edge_occt.borrow();
        let edge_shape = edge_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null edge" })?;
        op.pin_mut().add_edge(radius, TopoDS::Edge(edge_shape));
    }
    // `Shape()` triggers the build; `IsDone()` is meaningful afterwards.
    {
        let _ = op.pin_mut().Shape();
    }
    if !op.IsDone() {
        return Err(OcctError::NotDone {
            operation: "fillet",
        }
        .into());
    }

    let out_faces = {
        let built = op.pin_mut().Shape();
        sub_shape_map(built, TopAbs_ShapeEnum::TopAbs_FACE)
    };
    let mut history = History::default();
    for edge in edges {
        let edge_occt = downcast(&edge.shape)?;
        let edge_guard = edge_occt.borrow();
        let edge_shape = edge_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null edge" })?;
        let lineage = edge_lineage(
            edge,
            edge_shape,
            &out_faces,
            |shape, modified, generated| {
                bridge::fc_fillet_modified(op.pin_mut(), shape, modified.pin_mut());
                bridge::fc_fillet_generated(op.pin_mut(), shape, generated.pin_mut());
                bridge::fc_fillet_is_deleted(op.pin_mut(), shape)
            },
        );
        merge_into(&mut history, lineage);
    }

    let result = {
        let built = op.pin_mut().Shape();
        erase(built)
    };
    Ok(OpResult::new(result, history))
}

/// Chamfer `edges`; `d2` selects the two-distance form.
pub(crate) fn chamfer(
    shape: &Shape,
    edges: &[ElementRef],
    d1: f64,
    d2: Option<f64>,
) -> Result<OpResult, GeomError> {
    if edges.is_empty() {
        return Err(OcctError::InvalidInput {
            what: "chamfer needs at least one edge",
        }
        .into());
    }
    let occt = downcast(shape)?;
    let guard = occt.borrow();
    let s = guard
        .as_ref()
        .ok_or(OcctError::InvalidInput { what: "null shape" })?;

    let mut op = ffi::b_rep_fillet_api::BRepFilletAPI_MakeChamfer_new(s);
    for edge in edges {
        let edge_occt = downcast(&edge.shape)?;
        let edge_guard = edge_occt.borrow();
        let edge_shape = edge_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null edge" })?;
        match d2 {
            Some(d2) => {
                let face = ancestors(shape, edge, ElementKind::Face)
                    .into_iter()
                    .next()
                    .ok_or(OcctError::InvalidInput {
                        what: "asymmetric chamfer needs an adjacent face",
                    })?;
                let face_occt = downcast(&face.shape)?;
                let face_guard = face_occt.borrow();
                let face_shape = face_guard
                    .as_ref()
                    .ok_or(OcctError::InvalidInput { what: "null face" })?;
                bridge::fc_chamfer_add_asym(
                    op.pin_mut(),
                    d1,
                    d2,
                    TopoDS::Edge(edge_shape),
                    TopoDS::Face(face_shape),
                );
            }
            None => op.pin_mut().add_edge(d1, TopoDS::Edge(edge_shape)),
        }
    }
    {
        let _ = op.pin_mut().Shape();
    }
    if !op.IsDone() {
        return Err(OcctError::NotDone {
            operation: "chamfer",
        }
        .into());
    }

    let out_faces = {
        let built = op.pin_mut().Shape();
        sub_shape_map(built, TopAbs_ShapeEnum::TopAbs_FACE)
    };
    let mut history = History::default();
    for edge in edges {
        let edge_occt = downcast(&edge.shape)?;
        let edge_guard = edge_occt.borrow();
        let edge_shape = edge_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null edge" })?;
        let lineage = edge_lineage(
            edge,
            edge_shape,
            &out_faces,
            |shape, modified, generated| {
                bridge::fc_chamfer_modified(op.pin_mut(), shape, modified.pin_mut());
                bridge::fc_chamfer_generated(op.pin_mut(), shape, generated.pin_mut());
                bridge::fc_chamfer_is_deleted(op.pin_mut(), shape)
            },
        );
        merge_into(&mut history, lineage);
    }

    let result = {
        let built = op.pin_mut().Shape();
        erase(built)
    };
    Ok(OpResult::new(result, history))
}
