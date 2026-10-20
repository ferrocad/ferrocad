//! # FerroCAD OCCT backend (`ferrocad_occt`)
//!
//! The real geometry kernel behind the [`ferrocad_geom`] seam. It implements
//! [`GeometryBackend`](ferrocad_geom::GeometryBackend) with OCCT
//! via [`opencascade_sys`](https://crates.io/crates/opencascade-sys), plus a
//! **sibling cxx bridge** (`include/fc_history.hxx` and `include/fc_topology.hxx`)
//! that adds the shape-history, tessellation-adjacency and chamfer calls the crate
//! does not bridge — no fork, no patch.
//!
//! The seam means nothing below the application depends on this crate: the document
//! core talks to a `dyn GeometryBackend`, and the application installs
//! [`OcctBackend`] at startup. See `docs/occt-integration.md`.
//!
//! # Building
//!
//! This crate needs OCCT 7.8+ and is **not** in the workspace's `default-members`,
//! so an ordinary `cargo build`/`cargo test` does not require a kernel. To build it,
//! point two environment variables at an OCCT prefix (see `docs/occt-bundling.md`):
//!
//! ```sh
//! export OCCT_INCLUDE_DIR=<prefix>/include/opencascade   # for build.rs
//! export OpenCASCADE_DIR=<prefix>/lib/cmake/opencascade  # for opencascade-sys
//! export LD_LIBRARY_PATH=<prefix>/lib:$LD_LIBRARY_PATH   # for running tests
//! cargo test -p ferrocad_occt
//! ```
//!
//! # Example
//!
//! ```
//! use ferrocad_geom::GeometryBackend;
//! use ferrocad_occt::OcctBackend;
//!
//! let backend = OcctBackend::new();
//! let a = backend.make_box(10.0, 10.0, 10.0).unwrap();
//! let b = backend.make_box(10.0, 10.0, 10.0).unwrap();
//! let fused = backend.fuse(&a, &b).unwrap();
//! assert!(backend.resolve(&fused.shape, "Face1").is_some());
//! ```

mod backend;
mod bridge;
mod error;
mod shape;
mod topology;

pub use backend::OcctBackend;
pub use error::OcctError;
pub use shape::{read_brep, write_brep, OcctShape};

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_geom::{
        Capabilities, Continuity, ElementKind, GeometryBackend, TessellationOptions,
    };

    #[test]
    fn make_box_has_six_faces() {
        let backend = OcctBackend::new();
        let cube = backend.make_box(10.0, 10.0, 10.0).unwrap();
        assert!(backend.resolve(&cube, "Face1").is_some());
        assert!(backend.resolve(&cube, "Face6").is_some());
        assert!(backend.resolve(&cube, "Face7").is_none());
    }

    #[test]
    fn a_box_tessellates_with_part_identity() {
        let backend = OcctBackend::new();
        let cube = backend.make_box(10.0, 10.0, 10.0).unwrap();
        let mesh = backend
            .tessellate(&cube, &TessellationOptions::default())
            .unwrap();

        assert_eq!(mesh.parts.len(), 6 + 12 + 8);
        assert_eq!(mesh.triangle_count(), 12);
        assert_eq!(mesh.triangle_parts.len(), 12);
        assert!(mesh.triangle_parts.iter().all(|part| *part < 6));

        // Each box edge samples to two points, so one segment per edge.
        assert_eq!(mesh.segment_count(), 12);
        assert!(mesh.segment_parts.iter().all(|part| (6..18).contains(part)));

        assert_eq!(mesh.vertices.len(), 8);
        assert_eq!(mesh.vertex_parts.len(), 8);
        assert!(mesh.vertex_parts.iter().all(|part| (18..26).contains(part)));

        assert_eq!(mesh.part_name(0), Some("Face1"));
        assert_eq!(mesh.part_name(6), Some("Edge1"));
        assert_eq!(mesh.part_name(18), Some("Vertex1"));
        assert!(mesh.normals.is_empty(), "the mesh is flat-shaded");

        let positions = mesh.positions.len();
        assert!(
            mesh.triangles
                .iter()
                .all(|index| (*index as usize) < positions)
        );
        assert!(mesh.edges.iter().all(|index| (*index as usize) < positions));
        assert!(
            mesh.vertices
                .iter()
                .all(|index| (*index as usize) < positions)
        );

        assert!(
            backend
                .capabilities()
                .contains(Capabilities::TESSELLATION | Capabilities::TOPOLOGY)
        );
    }

    #[test]
    fn element_names_round_trip_through_resolve() {
        let backend = OcctBackend::new();
        let cube = backend.make_box(10.0, 10.0, 10.0).unwrap();

        for kind in [
            ElementKind::Solid,
            ElementKind::Shell,
            ElementKind::Face,
            ElementKind::Edge,
            ElementKind::Vertex,
        ] {
            let elements = backend.elements(&cube, kind);
            assert!(!elements.is_empty(), "{kind:?} has elements");
            for (index, element) in elements.iter().enumerate() {
                assert_eq!(element.name, format!("{}{}", kind.prefix(), index + 1));
                let resolved = backend
                    .resolve(&cube, &element.name)
                    .unwrap_or_else(|| panic!("{} resolves", element.name));
                let expected = crate::shape::downcast(&element.shape).unwrap();
                let actual = crate::shape::downcast(&resolved).unwrap();
                let expected = expected.borrow();
                let actual = actual.borrow();
                assert!(
                    expected.as_ref().unwrap().IsEqual(actual.as_ref().unwrap()),
                    "{} names the same sub-shape",
                    element.name
                );
            }
        }
    }

    #[test]
    fn ancestors_work_in_both_directions() {
        let backend = OcctBackend::new();
        let cube = backend.make_box(10.0, 10.0, 10.0).unwrap();
        let edges = backend.elements(&cube, ElementKind::Edge);
        let faces = backend.elements(&cube, ElementKind::Face);
        assert_eq!(edges.len(), 12);
        assert_eq!(faces.len(), 6);

        for edge in &edges {
            assert_eq!(
                backend.ancestors(&cube, edge, ElementKind::Face).len(),
                2,
                "{} has two adjacent faces",
                edge.name
            );
        }
        for face in &faces {
            assert_eq!(
                backend.ancestors(&cube, face, ElementKind::Edge).len(),
                4,
                "{} has four boundary edges",
                face.name
            );
        }
    }

    #[test]
    fn a_box_corner_is_c0() {
        let backend = OcctBackend::new();
        let cube = backend.make_box(10.0, 10.0, 10.0).unwrap();
        let face = backend.elements(&cube, ElementKind::Face)[0].clone();
        let edges = backend.ancestors(&cube, &face, ElementKind::Edge);
        assert_eq!(edges.len(), 4);

        // A rectangle's four sides meet at four corners; opposite pairs share no
        // vertex and report `None`.
        let mut corners = 0;
        for i in 0..edges.len() {
            for j in (i + 1)..edges.len() {
                if let Some(continuity) = backend.continuity(&cube, &edges[i], &edges[j]) {
                    assert_eq!(continuity, Continuity::C0);
                    corners += 1;
                }
            }
        }
        assert_eq!(corners, 4, "four adjacent edge pairs");
    }

    #[test]
    fn chamfer_reports_lineage() {
        let backend = OcctBackend::new();
        let cube = backend.make_box(10.0, 10.0, 10.0).unwrap();
        let edge = backend.elements(&cube, ElementKind::Edge)[0].clone();

        let out = backend
            .chamfer(&cube, std::slice::from_ref(&edge), 1.0, None)
            .unwrap();
        assert_eq!(backend.elements(&out.shape, ElementKind::Face).len(), 7);
        assert!(!out.history.is_empty(), "a chamfer changes the topology");
        assert!(
            out.history
                .deleted
                .iter()
                .any(|deleted| deleted.name == edge.name),
            "the seeded edge is consumed"
        );
        assert!(backend.capabilities().contains(Capabilities::CHAMFER));
    }

    #[test]
    fn asymmetric_chamfer_builds() {
        let backend = OcctBackend::new();
        let cube = backend.make_box(10.0, 10.0, 10.0).unwrap();
        let edge = backend.elements(&cube, ElementKind::Edge)[0].clone();
        let out = backend
            .chamfer(&cube, std::slice::from_ref(&edge), 1.0, Some(2.0))
            .unwrap();
        assert_eq!(backend.elements(&out.shape, ElementKind::Face).len(), 7);
    }

    #[test]
    fn fillet_reports_generated_faces() {
        let backend = OcctBackend::new();
        let cube = backend.make_box(10.0, 10.0, 10.0).unwrap();
        let edge = backend.elements(&cube, ElementKind::Edge)[0].clone();
        let out = backend
            .fillet(&cube, std::slice::from_ref(&edge), 1.0)
            .unwrap();
        assert_eq!(backend.elements(&out.shape, ElementKind::Face).len(), 7);
        assert!(
            out.history
                .generated
                .iter()
                .any(|(input, _)| input.name == edge.name),
            "the fillet face is generated from the edge"
        );
        assert!(
            out.history
                .deleted
                .iter()
                .any(|deleted| deleted.name == edge.name)
        );
    }

    #[test]
    fn a_placed_box_tessellates_at_its_location() {
        use ferrocad_types::{Placement, Rotation, Vector3};
        let backend = OcctBackend::new();
        let cube = backend.make_box(10.0, 10.0, 10.0).unwrap();
        let moved = backend
            .place(
                &cube,
                &Placement {
                    base: Vector3::new(10.0, 0.0, 0.0),
                    rotation: Rotation::identity(),
                },
            )
            .unwrap();
        let mesh = backend
            .tessellate(&moved, &TessellationOptions::absolute(0.5))
            .unwrap();
        let min_x = mesh
            .positions
            .iter()
            .map(|position| position[0])
            .fold(f32::INFINITY, f32::min);
        assert!(
            (min_x - 10.0).abs() < 1e-3,
            "location applied: min x = {min_x}"
        );
    }

    #[test]
    fn fuse_reports_face_lineage() {
        let backend = OcctBackend::new();
        let a = backend.make_box(10.0, 10.0, 10.0).unwrap();
        let b = backend.make_box(10.0, 10.0, 10.0).unwrap();
        let out = backend.fuse(&a, &b).unwrap();
        assert!(!out.history.is_empty());
        assert!(out.history.deleted.is_empty());
    }

    #[test]
    fn cut_removes_material() {
        let backend = OcctBackend::new();
        let a = backend.make_box(10.0, 10.0, 10.0).unwrap();
        let b = backend.make_box(10.0, 10.0, 10.0).unwrap();
        let out = backend.cut(&a, &b).unwrap();
        assert!(!out.history.is_empty());
    }

    #[test]
    fn place_and_resolve() {
        use ferrocad_types::{Placement, Rotation, Vector3};
        let backend = OcctBackend::new();
        let cube = backend.make_box(1.0, 1.0, 1.0).unwrap();
        let moved = backend
            .place(
                &cube,
                &Placement {
                    base: Vector3::new(5.0, 0.0, 0.0),
                    rotation: Rotation::identity(),
                },
            )
            .unwrap();
        assert!(backend.resolve(&moved, "Face1").is_some());
        assert!(!cube.is_same(&moved));
    }

    #[test]
    fn rejects_a_foreign_shape() {
        use ferrocad_geom::NullBackend;
        let occt = OcctBackend::new();
        let foreign = NullBackend.make_box(1.0, 1.0, 1.0).unwrap();
        assert_eq!(occt.resolve(&foreign, "Face1"), None);
    }
}
