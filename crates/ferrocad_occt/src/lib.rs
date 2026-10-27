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
//! [`OcctBackend`] at startup. See `docs/architecture/occt-integration.md`.
//!
//! # Building
//!
//! This crate needs OCCT 7.8+ and is **not** in the workspace's `default-members`,
//! so an ordinary `cargo build`/`cargo test` does not require a kernel. The workspace
//! `.cargo/config.toml` points at `.occt/prefix` automatically; with a custom prefix,
//! export (see `docs/architecture/occt-bundling.md`):
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
//! ```ignore
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
pub mod chfi2d;
mod error;
mod shape;
pub mod part_ops;
pub mod shapefix;
pub mod surface_ops;
mod topology;

pub use backend::OcctBackend;
pub use backend::surface_spec;
pub use error::OcctError;
pub use shape::{read_brep, write_brep, OcctShape};

/// The OCCT release this backend was compiled against, e.g. `"7.8.1"`.
///
/// Read from `OCC_VERSION_COMPLETE` in the headers `build.rs` located, so it
/// cannot drift from the kernel that is actually linked. The Python surface
/// reports it as `Part.OCC_VERSION`, which upstream's Draft reads to decide
/// whether hidden-line removal is available.
pub const OCC_VERSION: &str = env!("FERROCAD_OCC_VERSION");

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
    fn a_box_edge_has_no_tangent_chain() {
        let backend = OcctBackend::new();
        let cube = backend.make_box(10.0, 10.0, 10.0).unwrap();
        let edge = backend.elements(&cube, ElementKind::Edge)[0].clone();
        // Every box edge meets its neighbours at a sharp corner.
        let chain = ferrocad_geom::tangent_chain(&backend, &cube, &edge);
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].name, edge.name);
    }

    #[test]
    fn a_fillet_leaves_a_tangent_chain_at_its_boundary() {
        let backend = OcctBackend::new();
        let cube = backend.make_box(10.0, 10.0, 10.0).unwrap();
        let edge = backend.elements(&cube, ElementKind::Edge)[0].clone();
        let out = backend
            .fillet(&cube, std::slice::from_ref(&edge), 2.0)
            .unwrap()
            .shape;
        let edges = backend.elements(&out, ElementKind::Edge);

        // The new cylindrical face meets the trimmed planes tangentially, so at least
        // one adjacent edge pair is `G1`.
        let mut tangent = None;
        for i in 0..edges.len() {
            for j in (i + 1)..edges.len() {
                if backend.continuity(&out, &edges[i], &edges[j]) == Some(Continuity::G1) {
                    tangent = Some((i, j));
                }
            }
        }
        let (i, j) = tangent.expect("the fillet boundary is tangent");

        let chain = ferrocad_geom::tangent_chain(&backend, &out, &edges[i]);
        assert!(
            chain.iter().any(|chained| chained.name == edges[j].name),
            "the smooth neighbour joins the chain"
        );
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
    fn an_impossible_fillet_reports_not_done_instead_of_aborting() {
        let backend = OcctBackend::new();
        let cube = backend.make_box(10.0, 10.0, 10.0).unwrap();
        let edge = backend.elements(&cube, ElementKind::Edge)[0].clone();
        // A radius far larger than the box cannot be built. OCCT's `Shape()` raises
        // `StdFail_NotDone` in this situation, so the seam has to `Build()` and check
        // `IsDone()` first; calling `Shape()` first aborted the process.
        let error = backend
            .fillet(&cube, std::slice::from_ref(&edge), 1.0e6)
            .expect_err("an impossible fillet is an error, not a crash");
        assert!(
            error.to_string().contains("fillet"),
            "the error names the operation: {error}"
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

    #[test]
    fn a_line_segment_makes_an_edge_and_reports_its_basis_line() {
        use ferrocad_curves::{EdgeCurve, LineSegment, Vector3};
        let backend = OcctBackend::new();
        let segment = EdgeCurve::LineSegment(
            LineSegment::new(Vector3::new(1.0, 2.0, 3.0), Vector3::new(4.0, 2.0, 3.0)).unwrap(),
        );
        let edge = backend.make_edge(&segment).unwrap();
        let curve = backend.curve_of(&edge).unwrap();
        let EdgeCurve::Line(line) = curve else {
            panic!("a segment's basis curve is a line, got {curve:?}");
        };
        assert!(line.distance_to(Vector3::new(1.0, 2.0, 3.0)) < 1e-9);
        assert!(line.distance_to(Vector3::new(4.0, 2.0, 3.0)) < 1e-9);
        assert!((line.direction().sub(&Vector3::new(1.0, 0.0, 0.0))).length() < 1e-9);
    }

    #[test]
    fn a_circle_round_trips_through_an_edge() {
        use ferrocad_curves::{Circle, EdgeCurve, Vector3};
        let backend = OcctBackend::new();
        let circle = Circle::new(
            Vector3::new(1.0, 2.0, 3.0),
            Vector3::new(0.0, 0.0, 1.0),
            5.0,
        )
        .unwrap();
        let edge = backend.make_edge(&EdgeCurve::Circle(circle)).unwrap();
        let EdgeCurve::Circle(back) = backend.curve_of(&edge).unwrap() else {
            panic!("a full circle edge is a circle");
        };
        assert!((back.center().sub(&circle.center())).length() < 1e-9);
        assert!((back.radius() - 5.0).abs() < 1e-9);
        assert!((back.normal().sub(&Vector3::new(0.0, 0.0, 1.0))).length() < 1e-9);
    }

    #[test]
    fn an_arc_round_trips_with_its_sweep_and_endpoints() {
        use ferrocad_curves::{Arc, Circle, EdgeCurve, Vector3};
        use std::f64::consts::FRAC_PI_2;
        let backend = OcctBackend::new();
        let circle = Circle::new(Vector3::zero(), Vector3::new(0.0, 0.0, 1.0), 2.0).unwrap();
        let arc = Arc::new(circle, 0.0, FRAC_PI_2).unwrap();
        let edge = backend.make_edge(&EdgeCurve::Arc(arc)).unwrap();
        let EdgeCurve::Arc(back) = backend.curve_of(&edge).unwrap() else {
            panic!("a trimmed arc edge is an arc");
        };
        assert!((back.circle().radius() - 2.0).abs() < 1e-9);
        assert!((back.sweep().abs() - FRAC_PI_2).abs() < 1e-9);
        assert!(back.start().distance(&arc.start()) < 1e-9);
        assert!(back.end().distance(&arc.end()) < 1e-9);
    }

    #[test]
    fn make_edge_builds_the_bounded_kinds_and_rejects_an_infinite_line() {
        use ferrocad_curves::{EdgeCurve, Ellipse, Line, Vector3};
        let backend = OcctBackend::new();
        let line =
            EdgeCurve::Line(Line::new(Vector3::zero(), Vector3::new(1.0, 0.0, 0.0)).unwrap());
        assert!(backend.make_edge(&line).is_err());
        let ellipse = EdgeCurve::Ellipse(
            Ellipse::new(Vector3::zero(), Vector3::new(0.0, 0.0, 1.0), 3.0, 1.0).unwrap(),
        );
        let edge = backend
            .make_edge(&ellipse)
            .expect("an ellipse now has an exact gp_Elips edge");
        let EdgeCurve::Ellipse(back) = backend.curve_of(&edge).unwrap() else {
            panic!("an elliptical edge reports its basis ellipse");
        };
        assert!((back.major_radius() - 3.0).abs() < 1e-9);
        assert!((back.minor_radius() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn an_elliptical_arc_round_trips_with_its_bounds() {
        use ferrocad_curves::{EdgeCurve, Ellipse, Vector3};
        let backend = OcctBackend::new();
        let ellipse = Ellipse::with_axis(
            Vector3::new(1.0, 2.0, 3.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
            3.0,
            1.0,
        )
        .unwrap();
        let arc = ellipse.trim(0.25, 1.5).unwrap();
        let edge = backend.make_edge(&EdgeCurve::EllipticalArc(arc)).unwrap();
        let EdgeCurve::EllipticalArc(back) = backend.curve_of(&edge).unwrap() else {
            panic!("a trimmed ellipse edge is an elliptical arc");
        };
        assert!((back.first() - 0.25).abs() < 1e-9);
        assert!((back.last() - 1.5).abs() < 1e-9);
        assert!((back.ellipse().major_radius() - 3.0).abs() < 1e-9);
        assert!((back.ellipse().minor_radius() - 1.0).abs() < 1e-9);
        assert!(back.start().distance(&arc.start()) < 1e-9);
        assert!(back.end().distance(&arc.end()) < 1e-9);
    }

    #[test]
    fn a_hyperbola_arc_round_trips_with_its_frame_and_bounds() {
        use ferrocad_curves::{ArcOfHyperbola, EdgeCurve, Hyperbola, Vector3};
        let backend = OcctBackend::new();
        let hyperbola = Hyperbola::with_axis(
            Vector3::new(1.0, 2.0, 3.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
            2.0,
            1.0,
        )
        .unwrap();
        let arc = ArcOfHyperbola::new(hyperbola, -0.5, 0.75).unwrap();
        let edge = backend.make_edge(&EdgeCurve::ArcOfHyperbola(arc)).unwrap();
        let EdgeCurve::ArcOfHyperbola(back) = backend.curve_of(&edge).unwrap() else {
            panic!("a trimmed hyperbola edge is an arc of hyperbola");
        };
        assert!((back.first() - -0.5).abs() < 1e-9);
        assert!((back.last() - 0.75).abs() < 1e-9);
        assert!((back.hyperbola().major_radius() - 2.0).abs() < 1e-9);
        assert!((back.hyperbola().minor_radius() - 1.0).abs() < 1e-9);
        assert!(back.start().distance(&arc.start()) < 1e-9);
        assert!(back.end().distance(&arc.end()) < 1e-9);
    }

    #[test]
    fn a_parabola_arc_round_trips_with_its_frame_and_bounds() {
        use ferrocad_curves::{ArcOfParabola, EdgeCurve, Parabola, Vector3};
        let backend = OcctBackend::new();
        let parabola = Parabola::with_axis(
            Vector3::new(1.0, 2.0, 3.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
            0.5,
        )
        .unwrap();
        let arc = ArcOfParabola::new(parabola, -1.0, 2.0).unwrap();
        let edge = backend.make_edge(&EdgeCurve::ArcOfParabola(arc)).unwrap();
        let EdgeCurve::ArcOfParabola(back) = backend.curve_of(&edge).unwrap() else {
            panic!("a trimmed parabola edge is an arc of parabola");
        };
        assert!((back.first() - -1.0).abs() < 1e-9);
        assert!((back.last() - 2.0).abs() < 1e-9);
        assert!((back.parabola().focal() - 0.5).abs() < 1e-9);
        assert!(back.start().distance(&arc.start()) < 1e-9);
        assert!(back.end().distance(&arc.end()) < 1e-9);
    }

    #[test]
    fn a_bezier_edge_round_trips_with_poles_weights_and_bounds() {
        use ferrocad_curves::{BezierCurve, EdgeCurve, Vector3};
        let backend = OcctBackend::new();
        let poles = vec![
            Vector3::new(0.0, 0.0, 0.0),
            Vector3::new(1.0, 2.0, 0.0),
            Vector3::new(2.0, 0.0, 0.0),
        ];
        let bezier = BezierCurve::new(poles.clone(), vec![1.0, 1.0, 1.0]).unwrap();
        let edge = backend.make_edge(&EdgeCurve::BezierCurve(bezier.clone())).unwrap();
        let EdgeCurve::BezierCurve(back) = backend.curve_of(&edge).unwrap() else {
            panic!("a Bezier edge reports a Bezier curve");
        };
        assert_eq!(back.poles(), poles.as_slice());
        assert_eq!(back.weights(), &[1.0, 1.0, 1.0]);
        assert!((back.first() - 0.0).abs() < 1e-12);
        assert!((back.last() - 1.0).abs() < 1e-12);
        for parameter in [0.0, 0.25, 0.5, 0.75, 1.0] {
            assert!(back.point_at(parameter).distance(&bezier.point_at(parameter)) < 1e-9);
        }
    }

    #[test]
    fn a_rational_bspline_edge_round_trips_with_its_data_and_bounds() {
        use ferrocad_curves::{EdgeCurve, NurbsCurve, Vector3};
        let backend = OcctBackend::new();
        let curve = NurbsCurve::new(
            2,
            vec![0.0, 2.0],
            vec![3, 3],
            vec![
                Vector3::new(0.0, 0.0, 0.0),
                Vector3::new(1.0, 1.0, 0.0),
                Vector3::new(2.0, 0.0, 0.0),
            ],
            vec![1.0, 0.5, 1.0],
        )
        .unwrap();
        let edge = backend.make_edge(&EdgeCurve::BSplineCurve(curve.clone())).unwrap();
        let EdgeCurve::BSplineCurve(back) = backend.curve_of(&edge).unwrap() else {
            panic!("a B-spline edge reports a B-spline curve");
        };
        assert_eq!(back.degree(), 2);
        assert_eq!(back.poles(), curve.poles());
        assert_eq!(back.weights(), curve.weights());
        assert_eq!(back.knots(), &[0.0, 2.0]);
        assert!((back.first() - 0.0).abs() < 1e-12);
        assert!((back.last() - 2.0).abs() < 1e-12);
        for parameter in [0.0, 0.5, 1.0, 1.5, 2.0] {
            assert!(back.point_at(parameter).unwrap().distance(&curve.point_at(parameter).unwrap()) < 1e-9);
        }
    }

    #[test]
    fn a_trimmed_bspline_edge_keeps_the_trim_range() {
        use ferrocad_curves::{EdgeCurve, NurbsCurve, Vector3};
        let backend = OcctBackend::new();
        let curve = NurbsCurve::new(
            2,
            vec![0.0, 2.0],
            vec![3, 3],
            vec![
                Vector3::new(0.0, 0.0, 0.0),
                Vector3::new(1.0, 1.0, 0.0),
                Vector3::new(2.0, 0.0, 0.0),
            ],
            vec![1.0, 1.0, 1.0],
        )
        .unwrap();
        let edge = backend
            .make_edge(&EdgeCurve::BSplineCurve(curve))
            .unwrap();
        let pieces = backend.split_edge(&edge, &[0.7, 1.3]).unwrap();
        let edges = backend.elements(&pieces, ElementKind::Edge);
        assert_eq!(edges.len(), 3);
        let EdgeCurve::BSplineCurve(back) = backend.curve_of(&edges[1].shape).unwrap() else {
            panic!("a trimmed B-spline edge still reports a B-spline");
        };
        assert!((back.first() - 0.7).abs() < 1e-9, "first was {}", back.first());
        assert!((back.last() - 1.3).abs() < 1e-9, "last was {}", back.last());
    }

    #[test]
    fn an_offset_curve_edge_round_trips_with_its_basis_and_offset() {
        use ferrocad_curves::{Circle, EdgeCurve, OffsetCurve, Vector3};
        let backend = OcctBackend::new();
        let circle = Circle::new(Vector3::zero(), Vector3::new(0.0, 0.0, 1.0), 2.0).unwrap();
        let basis = EdgeCurve::Circle(circle);
        let offset = OffsetCurve::new(
            basis.clone(),
            0.5,
            Vector3::new(0.0, 0.0, 1.0),
            0.0,
            std::f64::consts::TAU,
        )
        .unwrap();
        let edge = backend.make_edge(&EdgeCurve::OffsetCurve(offset.clone())).unwrap();
        let EdgeCurve::OffsetCurve(back) = backend.curve_of(&edge).unwrap() else {
            panic!("an offset edge reports an offset curve");
        };
        assert!((back.offset() - 0.5).abs() < 1e-9);
        assert!(back.direction().sub(&Vector3::new(0.0, 0.0, 1.0)).length() < 1e-9);
        assert_eq!(back.basis().kind(), "Circle");
        assert!((back.first() - 0.0).abs() < 1e-9);
        assert!((back.last() - std::f64::consts::TAU).abs() < 1e-9);
        assert!((back.point_at(0.0).length() - 2.5).abs() < 1e-9);
    }

    #[test]
    fn curve_of_a_non_edge_is_unsupported() {
        let backend = OcctBackend::new();
        let cube = backend.make_box(1.0, 1.0, 1.0).unwrap();
        assert!(backend.curve_of(&cube).is_err());
    }
}
