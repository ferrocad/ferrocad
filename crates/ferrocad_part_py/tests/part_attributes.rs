//! The kernel-independent `Part.Shape` introspection attributes that upstream's
//! `TopoShapeTest.testTopoShapeBox` asserts on
//! (`src/Mod/Part/parttests/TopoShapeTest.py`).
//!
//! Same boot as `part_unified`: `ferrocad` + `Part` as built-ins, one core, one
//! OCCT backend. It needs OCCT (see `docs/architecture/occt-bundling.md`).

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyModule};

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

/// Upstream's `Box2`: `Part.makeBox(2, 1, 2)`, volume 4.
fn make_box<'py>(part: &Bound<'py, PyModule>) -> Bound<'py, PyAny> {
    part.call_method1("makeBox", (2.0, 1.0, 2.0))
        .expect("Part.makeBox(2, 1, 2)")
}

fn text(shape: &Bound<'_, PyAny>, attr: &str) -> String {
    shape
        .getattr(attr)
        .unwrap_or_else(|_| panic!("Shape.{attr}"))
        .extract::<String>()
        .unwrap_or_else(|_| panic!("Shape.{attr} as a string"))
}

fn number(shape: &Bound<'_, PyAny>, attr: &str) -> f64 {
    shape
        .getattr(attr)
        .unwrap_or_else(|_| panic!("Shape.{attr}"))
        .extract::<f64>()
        .unwrap_or_else(|_| panic!("Shape.{attr} as f64"))
}

fn count(shape: &Bound<'_, PyAny>, attr: &str) -> usize {
    shape
        .getattr(attr)
        .unwrap_or_else(|_| panic!("Shape.{attr}"))
        .len()
        .unwrap_or_else(|_| panic!("len(Shape.{attr})"))
}

fn assert_close(actual: f64, expected: f64, what: &str) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "{what} is {actual}, expected {expected}"
    );
}

/// The attributes `testTopoShapeBox` string-matches, except the ones that need
/// kernel data (`MatrixOfInertia`, `PrincipalProperties`) and the inert ones
/// (`ElementMapVersion`, `ElementMapSize`).
#[test]
fn box_introspection_attributes() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let shape = make_box(&part);

        assert_eq!(text(&shape, "ShapeType"), "Solid");
        assert_eq!(text(&shape, "TypeId"), "Part::TopoShape");
        assert_eq!(text(&shape, "Module"), "Part");
        assert_eq!(text(&shape, "Orientation"), "Forward");
        assert_eq!(text(&shape, "Content"), "<ElementMap/>\n");

        // A fresh box has no history hasher upstream either.
        let hasher = shape.getattr("Hasher").expect("Shape.Hasher");
        assert!(hasher.is_none(), "Shape.Hasher should be None, got {hasher}");

        let element_map = shape.getattr("ElementMap").expect("Shape.ElementMap");
        let element_map = element_map
            .downcast::<PyDict>()
            .expect("Shape.ElementMap should be a dict");
        assert_eq!(element_map.len(), 0, "Shape.ElementMap should be empty");

        let reverse_map = shape
            .getattr("ElementReverseMap")
            .expect("Shape.ElementReverseMap");
        let reverse_map = reverse_map
            .downcast::<PyDict>()
            .expect("Shape.ElementReverseMap should be a dict");
        assert_eq!(reverse_map.len(), 0);

        // The seam has no enumeration for these kinds, so they stay empty.
        for attr in ["Compounds", "CompSolids"] {
            let list = shape.getattr(attr).unwrap_or_else(|_| panic!("Shape.{attr}"));
            let list = list
                .downcast::<PyList>()
                .unwrap_or_else(|_| panic!("Shape.{attr} should be a list"));
            assert_eq!(list.len(), 0, "Shape.{attr} should be empty");
        }

        // Left as they were: no TNP, no element map.
        assert_eq!(text(&shape, "ElementMapVersion"), "");
        assert_eq!(
            shape
                .getattr("ElementMapSize")
                .expect("Shape.ElementMapSize")
                .extract::<usize>()
                .expect("Shape.ElementMapSize as usize"),
            0
        );
    });
}

/// `testTopoShapeBox`'s numeric and count assertions.
#[test]
fn box_measurements() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let shape = make_box(&part);

        // Unit density: Mass is the volume.
        assert_close(number(&shape, "Volume"), 4.0, "Volume");
        assert_close(number(&shape, "Mass"), 4.0, "Mass");
        assert_close(number(&shape, "Area"), 16.0, "Area");

        // Upstream sums the edges of every face (each box edge belongs to two
        // faces), so 2 * (4*2 + 4*1 + 4*2) = 40, not 20.
        assert_close(number(&shape, "Length"), 40.0, "Length");

        let moments = shape
            .getattr("StaticMoments")
            .expect("Shape.StaticMoments")
            .extract::<(f64, f64, f64)>()
            .expect("Shape.StaticMoments as a tuple");
        assert_close(moments.0, 4.0, "StaticMoments[0]");
        assert_close(moments.1, 2.0, "StaticMoments[1]");
        assert_close(moments.2, 4.0, "StaticMoments[2]");

        assert_eq!(count(&shape, "Solids"), 1);
        assert_eq!(count(&shape, "Shells"), 1);
        // The sub-shape getters keep working alongside the new ones.
        assert_eq!(count(&shape, "Faces"), 6);
        assert_eq!(count(&shape, "Wires"), 6);
        assert_eq!(count(&shape, "Edges"), 12);
        assert_eq!(count(&shape, "Vertexes"), 8);

        let solid = shape
            .getattr("Solids")
            .expect("Shape.Solids")
            .get_item(0)
            .expect("the box's solid");
        assert_eq!(text(&solid, "ShapeType"), "Solid");
        let shell = shape
            .getattr("Shells")
            .expect("Shape.Shells")
            .get_item(0)
            .expect("the box's shell");
        assert_eq!(text(&shell, "ShapeType"), "Shell");
    });
}

/// An edge is its own kind and keeps the kernel's linear-properties length; the
/// solid's `Length` is the per-face sum of those edge lengths.
#[test]
fn edge_reports_its_own_kind_and_length() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let shape = make_box(&part);
        let edges = shape.getattr("Edges").expect("Shape.Edges");
        assert_eq!(edges.len().expect("edge count"), 12);

        let mut single_edge_lengths = 0.0;
        for index in 0..12 {
            let edge = edges.get_item(index).expect("an edge");
            // The wrapper is a `Part.Shape` subclass; `toShape` is the plain
            // kernel handle with the introspection getters.
            let handle = edge.call_method0("toShape").expect("Edge.toShape()");
            assert_eq!(text(&handle, "ShapeType"), "Edge");
            let length = number(&handle, "Length");
            assert_close(length, number(&edge, "Length"), "Edge.Length");
            assert!(length > 0.0, "an edge must have positive length");
            single_edge_lengths += length;
        }
        // Each box edge counted once: 4*2 + 4*1 + 4*2.
        assert_close(single_edge_lengths, 20.0, "sum of edge lengths");

        let wire = shape
            .getattr("Faces")
            .expect("Shape.Faces")
            .get_item(0)
            .expect("a face")
            .getattr("OuterWire")
            .expect("Face.OuterWire");
        assert_eq!(text(&wire, "ShapeType"), "Wire");
    });
}
