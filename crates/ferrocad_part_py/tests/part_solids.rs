//! Shells, solids and the solid-side `Part` façade, end to end.
//!
//! Covers `Part.makeSolid`, `Part.makeShell`, `Part.makeShellFromWires`,
//! `Part.Shell`/`Part.Solid`, `Part.makeLoft`, `Part.makeRuledSurface`,
//! `Part.makeFilledFace`, `Shape.extrude`/`Shape.revolve` and
//! `Part.getSortedClusters`.
//!
//! Same boot as `part_curves`: `ferrocad` + `Part` as built-ins, one core, one
//! OCCT backend. It needs OCCT (see `docs/developer/architecture/occt-bundling.md`).

use pyo3::prelude::*;
use pyo3::types::{PyList, PyModule};

/// Register both built-in modules before the interpreter initialises.
fn boot() {
    static BOOT: std::sync::Once = std::sync::Once::new();
    BOOT.call_once(|| {
        use ferrocad_part_py::Part as part_module;
        use ferrocad_py::ferrocad as ferrocad_module;
        pyo3::append_to_inittab!(ferrocad_module);
        pyo3::append_to_inittab!(part_module);
    });
}

/// A `ferrocad.Vector`.
fn vector<'py>(ferrocad: &Bound<'py, PyModule>, x: f64, y: f64, z: f64) -> Bound<'py, PyAny> {
    ferrocad
        .getattr("Vector")
        .expect("ferrocad.Vector")
        .call1((x, y, z))
        .expect("a vector")
}

/// A closed polygon wire on the XY plane.
fn square<'py>(
    part: &Bound<'py, PyModule>,
    ferrocad: &Bound<'py, PyModule>,
    corner: (f64, f64),
) -> Bound<'py, PyAny> {
    let (x, y) = corner;
    let points = PyList::new(
        part.py(),
        [
            vector(ferrocad, x, y, 0.0),
            vector(ferrocad, x + 1.0, y, 0.0),
            vector(ferrocad, x + 1.0, y + 1.0, 0.0),
            vector(ferrocad, x, y + 1.0, 0.0),
        ],
    )
    .expect("point list");
    part.call_method1("makePolygon", (&points, true))
        .expect("Part.makePolygon(points, closed=True)")
}

fn is_null(shape: &Bound<'_, PyAny>) -> bool {
    shape
        .call_method0("isNull")
        .expect("Shape.isNull")
        .extract::<bool>()
        .expect("isNull as bool")
}

fn volume(shape: &Bound<'_, PyAny>) -> f64 {
    shape
        .getattr("Volume")
        .expect("Shape.Volume")
        .extract::<f64>()
        .expect("volume as f64")
}

fn area(shape: &Bound<'_, PyAny>) -> f64 {
    shape
        .getattr("Area")
        .expect("Shape.Area")
        .extract::<f64>()
        .expect("area as f64")
}

fn count(shape: &Bound<'_, PyAny>, name: &str) -> usize {
    shape
        .getattr(name)
        .unwrap_or_else(|_| panic!("Shape.{name}"))
        .len()
        .unwrap_or_else(|_| panic!("len(Shape.{name})"))
}

/// The `BoundBox` as `[xmin, ymin, zmin, xmax, ymax, zmax]`.
fn bounds(shape: &Bound<'_, PyAny>) -> [f64; 6] {
    let bound_box = shape.getattr("BoundBox").expect("Shape.BoundBox");
    let value = |name: &str| {
        bound_box
            .getattr(name)
            .unwrap_or_else(|_| panic!("BoundBox.{name}"))
            .extract::<f64>()
            .unwrap_or_else(|_| panic!("BoundBox.{name} as f64"))
    };
    [
        value("XMin"),
        value("YMin"),
        value("ZMin"),
        value("XMax"),
        value("YMax"),
        value("ZMax"),
    ]
}

fn assert_bounds(shape: &Bound<'_, PyAny>, expected: [f64; 6]) {
    let actual = bounds(shape);
    for (index, (have, want)) in actual.iter().zip(expected.iter()).enumerate() {
        assert!(
            (have - want).abs() < 1e-7,
            "bounds[{index}] was {have}, expected {want} (all: {actual:?})"
        );
    }
}

#[test]
fn make_solid_wraps_a_box_shell() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        // Upstream's Box1 is 1 x 2 x 2.
        let cube = part.call_method1("makeBox", (1.0, 2.0, 2.0)).expect("box");
        let shell = cube.getattr("Shell1").expect("Shape.Shell1");

        let solid = part
            .call_method1("makeSolid", (&shell,))
            .expect("Part.makeSolid(shell)");
        assert!(!is_null(&solid), "a solid from a closed shell is not null");
        assert_eq!(count(&solid, "Shells"), 1);
        assert!((volume(&solid) - 4.0).abs() < 1e-7, "volume was {}", volume(&solid));
        assert_bounds(&solid, [0.0, 0.0, 0.0, 1.0, 2.0, 2.0]);
    });
}

#[test]
fn make_shell_groups_faces() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let cube = part.call_method1("makeBox", (1.0, 2.0, 2.0)).expect("box");
        let faces = cube.getattr("Faces").expect("Shape.Faces");

        let shell = part
            .call_method1("makeShell", (&faces,))
            .expect("Part.makeShell(faces)");
        assert!(!is_null(&shell), "a shell of six faces is not null");
        assert_eq!(count(&shell, "Faces"), 6);
        assert_eq!(count(&shell, "Shells"), 1);
        assert_bounds(&shell, [0.0, 0.0, 0.0, 1.0, 2.0, 2.0]);
    });
}

#[test]
fn make_shell_from_wires_uses_one_face_per_wire() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let cube = part.call_method1("makeBox", (1.0, 2.0, 2.0)).expect("box");
        let wires = cube.getattr("Wires").expect("Shape.Wires");
        let first_two = PyList::new(
            py,
            [
                wires.get_item(0).expect("Wire1"),
                wires.get_item(1).expect("Wire2"),
            ],
        )
        .expect("wire list");

        let shell = part
            .call_method1("makeShellFromWires", (&first_two,))
            .expect("Part.makeShellFromWires(wires)");
        assert!(!is_null(&shell), "a shell from two wires is not null");
        assert_eq!(count(&shell, "Faces"), 2);
        assert_eq!(count(&shell, "Shells"), 1);
        assert_bounds(&shell, [0.0, 0.0, 0.0, 1.0, 2.0, 2.0]);
    });
}

#[test]
fn shell_and_solid_classes_start_null_and_accept_shapes() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let cube = part.call_method1("makeBox", (1.0, 2.0, 2.0)).expect("box");

        let shell_class = part.getattr("Shell").expect("Part.Shell");
        let shell = shell_class.call0().expect("Part.Shell()");
        assert!(is_null(&shell), "Part.Shell() is the null shell");
        shell
            .call_method1("add", (cube.getattr("Face1").expect("Face1"),))
            .expect("add a face to an empty shell");
        assert!(!is_null(&shell), "a shell with a face is not null");
        assert_eq!(count(&shell, "Faces"), 1);
        assert_eq!(
            shell
                .getattr("ShapeType")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "Shell"
        );

        let solid_class = part.getattr("Solid").expect("Part.Solid");
        let solid = solid_class.call0().expect("Part.Solid()");
        assert!(is_null(&solid), "Part.Solid() is the null solid");
        solid
            .call_method1("add", (cube.getattr("Shell1").expect("Shell1"),))
            .expect("add a shell to an empty solid");
        assert!(!is_null(&solid), "a solid with a shell is not null");
        assert_eq!(
            solid
                .getattr("ShapeType")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "Solid"
        );
        assert!((volume(&solid) - 4.0).abs() < 1e-7);
    });
}

#[test]
fn make_loft_builds_through_two_wires() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let cube = part.call_method1("makeBox", (1.0, 2.0, 2.0)).expect("box");
        let wires = cube.getattr("Wires").expect("Shape.Wires");
        let first_two = PyList::new(
            py,
            [
                wires.get_item(0).expect("Wire1"),
                wires.get_item(1).expect("Wire2"),
            ],
        )
        .expect("wire list");

        let loft = part
            .call_method1("makeLoft", (&first_two,))
            .expect("Part.makeLoft(wires)");
        assert!(!is_null(&loft), "a loft through two wires is not null");
        assert_bounds(&loft, [0.0, 0.0, 0.0, 1.0, 2.0, 2.0]);
    });
}

#[test]
fn make_ruled_surface_spans_two_edges() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let cube = part.call_method1("makeBox", (1.0, 2.0, 2.0)).expect("box");
        let edges = cube.getattr("Edges").expect("Shape.Edges");
        // Upstream uses the box's edges 4 and 5 (`Edges[3:5]`).
        let surface = part
            .call_method1(
                "makeRuledSurface",
                (
                    edges.get_item(3).expect("Edge4"),
                    edges.get_item(4).expect("Edge5"),
                ),
            )
            .expect("Part.makeRuledSurface(edge, edge)");
        assert!(!is_null(&surface), "a ruled surface is not null");
        assert!(area(&surface) > 0.0, "a ruled surface has area");
    });
}

#[test]
fn make_filled_face_fills_a_face_boundary() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let cube = part.call_method1("makeBox", (1.0, 2.0, 2.0)).expect("box");
        let face = cube.getattr("Face1").expect("Shape.Face1");
        let edges = face.getattr("Edges").expect("Face.Edges");

        let filled = part
            .call_method1("makeFilledFace", (&edges,))
            .expect("Part.makeFilledFace(edges)");
        assert!(!is_null(&filled), "a filled face is not null");
        assert!(area(&filled) > 0.0, "a filled face has area");
    });
}

#[test]
fn extrude_and_revolve_a_planar_face() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let wire = square(&part, &ferrocad, (0.0, 0.0));
        let face = part
            .getattr("Face")
            .expect("Part.Face")
            .call1((&wire,))
            .expect("Part.Face(wire)");

        // Extrude one unit of face by two along +Z.
        let extruded = face
            .call_method1("extrude", (vector(&ferrocad, 0.0, 0.0, 2.0),))
            .expect("Shape.extrude");
        assert!(!is_null(&extruded), "an extruded face is not null");
        assert!(
            (volume(&extruded) - 2.0).abs() < 1e-7,
            "extruded volume was {}",
            volume(&extruded)
        );
        assert_bounds(&extruded, [0.0, 0.0, 0.0, 1.0, 1.0, 2.0]);

        // A full turn about the X axis sweeps the unit square into a cylinder.
        let revolved = face
            .call_method1(
                "revolve",
                (
                    vector(&ferrocad, 0.0, 0.0, 0.0),
                    vector(&ferrocad, 1.0, 0.0, 0.0),
                    360.0,
                ),
            )
            .expect("Shape.revolve");
        assert!(!is_null(&revolved), "a revolved face is not null");
        assert!(
            (volume(&revolved).abs() - std::f64::consts::PI).abs() < 1e-6,
            "revolved volume was {}",
            volume(&revolved)
        );
        assert_bounds(&revolved, [0.0, -1.0, -1.0, 1.0, 1.0, 1.0]);
    });
}

#[test]
fn get_sorted_clusters_groups_connected_edges() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");

        // A square's four edges are one cluster of four.
        let wire = square(&part, &ferrocad, (0.0, 0.0));
        let edges = wire.getattr("Edges").expect("Wire.Edges");
        let clusters = part
            .call_method1("getSortedClusters", (&edges,))
            .expect("Part.getSortedClusters(edges)");
        assert_eq!(clusters.len().expect("cluster count"), 1);
        let cluster = clusters.get_item(0).expect("the cluster");
        assert_eq!(cluster.len().expect("edge count"), 4);

        // Two separate squares are two clusters (four edges each).
        let far = square(&part, &ferrocad, (10.0, 0.0));
        let far_edges = far.getattr("Edges").expect("Wire.Edges");
        let mut combined_items = Vec::new();
        for edges in [&edges, &far_edges] {
            for index in 0..4 {
                combined_items.push(edges.get_item(index).expect("an edge"));
            }
        }
        let combined = PyList::new(py, combined_items).expect("combined edges");
        let clusters = part
            .call_method1("getSortedClusters", (&combined,))
            .expect("Part.getSortedClusters(disconnected edges)");
        assert_eq!(clusters.len().expect("cluster count"), 2);
        for index in 0..2 {
            assert_eq!(
                clusters
                    .get_item(index)
                    .expect("a cluster")
                    .len()
                    .expect("edge count"),
                4
            );
        }
    });
}
