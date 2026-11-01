//! Binary STL export over the OCCT triangulation.
//!
//! The old `fc_stl_bytes` bridge lived in the bindings crate and returned the
//! encoded bytes directly. Meshing is now Rust over the generic sys bindings and
//! the byte encoding is a pure-Rust [`StlMesh::encode_binary`], so the Python
//! surface keeps its `Vec<u8>` ABI while the C++ bridge shrinks.

use ferrocad_curves::Vector3;
use opencascade_sys as ffi;

use ffi::top_abs::{TopAbs_Orientation, TopAbs_ShapeEnum};
use ffi::topo_ds::TopoDS_Shape;

use crate::error::OcctError;
use crate::shape::sub_shape_map;

/// One oriented triangle. The winding is already flipped for reversed faces, so
/// the normal computed by the encoder points out of the solid.
pub struct StlFacet {
    pub vertices: [Vector3; 3],
}

/// The flat triangle soup of a shape, before the STL header/attribute bytes are
/// added.
pub struct StlMesh {
    pub facets: Vec<StlFacet>,
}

impl StlMesh {
    /// Encode this mesh as binary STL: an 80-byte header, the little-endian
    /// facet count, then one facet per triangle (12 floats plus a two-byte
    /// attribute count).
    pub fn encode_binary(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(84 + self.facets.len() * 50);
        out.extend_from_slice(&[0u8; 80]);
        out.extend_from_slice(&(self.facets.len() as u32).to_le_bytes());
        for facet in &self.facets {
            let normal = facet_normal(facet);
            push_f32(&mut out, normal.x as f32);
            push_f32(&mut out, normal.y as f32);
            push_f32(&mut out, normal.z as f32);
            for vertex in &facet.vertices {
                push_f32(&mut out, vertex.x as f32);
                push_f32(&mut out, vertex.y as f32);
                push_f32(&mut out, vertex.z as f32);
            }
            out.extend_from_slice(&[0u8, 0u8]);
        }
        out
    }
}

/// `Shape.exportStl`: mesh `shape` at `deflection` and return the oriented
/// triangles. The angular deflection formula and the face-orientation winding
/// flip are upstream's; the degenerate-facet drop is the same guard upstream's
/// writer relies on.
pub(crate) fn mesh(shape: &TopoDS_Shape, deflection: f64) -> Result<StlMesh, OcctError> {
    if shape.IsNull() || !deflection.is_finite() || deflection <= 0.0 {
        return Err(OcctError::InvalidInput {
            what: "the deflection is a positive number",
        });
    }

    let angular = 0.1_f64.min(deflection * 5.0 + 0.005);
    let mesher = ffi::b_rep_mesh::IncrementalMesh_new_parallel(
        shape,
        deflection,
        false,
        angular,
        true,
    );
    if !mesher.IsDone() {
        return Err(OcctError::NotDone {
            operation: "stl_bytes",
        });
    }

    let faces = sub_shape_map(shape, TopAbs_ShapeEnum::TopAbs_FACE);
    let confusion = ffi::precision::Precision::Confusion();
    let mut facets = Vec::new();

    for face_index in 1..=faces.Extent() {
        let face_shape = faces.FindKey(face_index);
        let face = ffi::topo_ds::TopoDS::Face(face_shape);
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

        for triangle_index in 1..=triangulation.NbTriangles() {
            let triangle = triangulation.Triangle(triangle_index);
            let mut pa = ffi::poly::Poly_Triangulation_Node(triangulation, triangle.Value(1));
            let mut pb = ffi::poly::Poly_Triangulation_Node(triangulation, triangle.Value(2));
            let mut pc = ffi::poly::Poly_Triangulation_Node(triangulation, triangle.Value(3));
            pa.pin_mut().Transform(&trsf);
            pb.pin_mut().Transform(&trsf);
            pc.pin_mut().Transform(&trsf);
            let pa = Vector3::new(pa.X(), pa.Y(), pa.Z());
            let pb = Vector3::new(pb.X(), pb.Y(), pb.Z());
            let pc = Vector3::new(pc.X(), pc.Y(), pc.Z());

            // A facet that collapses to a line is not a surface.
            if pa.distance(&pb) <= confusion
                || pb.distance(&pc) <= confusion
                || pc.distance(&pa) <= confusion
            {
                continue;
            }

            // OCCT keeps the face orientation beside the triangulation; flip the
            // winding so the stored normal points out of the solid.
            let vertices = if reversed { [pa, pc, pb] } else { [pa, pb, pc] };
            facets.push(StlFacet { vertices });
        }
    }

    Ok(StlMesh { facets })
}

fn facet_normal(facet: &StlFacet) -> Vector3 {
    let [a, b, c] = facet.vertices;
    let normal = b.sub(&a).cross(&c.sub(&a));
    let length = normal.length();
    if length > 0.0 {
        normal.scale(1.0 / length)
    } else {
        Vector3::new(0.0, 0.0, 0.0)
    }
}

fn push_f32(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_le_bytes());
}
