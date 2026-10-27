//! The shape-lifecycle operations the upstream Part tests lean on:
//! `mapSubElement`, `Edge.split`, `isValid`, `Part.makeSplitShape`,
//! `Part.show`, `Part.makeSweepSurface`, and the inertia getters.
//!
//! Same boot as `part_unified`: `ferrocad` + `Part` as built-ins, one core, one
//! OCCT backend. It needs OCCT (see `docs/developer/architecture/occt-bundling.md`).

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyModule, PyTuple};

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

fn number(value: &Bound<'_, PyAny>, what: &str) -> f64 {
    value
        .extract::<f64>()
        .unwrap_or_else(|_| panic!("{what} as f64"))
}

fn length_of(shape: &Bound<'_, PyAny>) -> f64 {
    number(&shape.getattr("Length").expect("Shape.Length"), "Length")
}

fn is_null(shape: &Bound<'_, PyAny>) -> bool {
    shape
        .call_method0("isNull")
        .expect("Shape.isNull")
        .extract::<bool>()
        .expect("isNull as bool")
}

fn element_map_size(shape: &Bound<'_, PyAny>) -> usize {
    shape
        .getattr("ElementMapSize")
        .expect("Shape.ElementMapSize")
        .extract::<usize>()
        .expect("ElementMapSize as usize")
}

/// `mapSubElement` is a documented no-op while this kernel emits no element
/// maps: the size stays 0 before and after, and the shape is returned.
#[test]
fn map_subelement_is_a_no_op() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let boxed = part
            .call_method1("makeBox", (1.0, 2.0, 3.0))
            .expect("Part.makeBox(1, 2, 3)");
        let face = boxed
            .getattr("Faces")
            .expect("Shape.Faces")
            .get_item(0)
            .expect("the first face");

        assert_eq!(element_map_size(&boxed), 0, "a fresh box has no element map");
        assert_eq!(element_map_size(&face), 0, "a fresh face has no element map");

        // Upstream's call shape: one sub-shape, optionally with an op code.
        let returned = boxed
            .call_method1("mapSubElement", (&face,))
            .expect("Shape.mapSubElement(face)");
        assert!(
            returned.is(&boxed),
            "mapSubElement returns the shape itself, as upstream does"
        );
        // And the list form.
        let faces = PyList::new(py, [&face]).expect("face list");
        boxed
            .call_method1("mapSubElement", (&faces, "Test"))
            .expect("Shape.mapSubElement([face], \"Test\")");

        assert_eq!(element_map_size(&boxed), 0, "the box's map stays empty");
        assert_eq!(element_map_size(&face), 0, "the face's map stays empty");
    });
}

/// `Edge.split` cuts an edge into one piece per parameter interval, in parameter
/// order, and leaves the original edge alone.
#[test]
fn edge_split_returns_the_pieces_in_order() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let edge = part
            .call_method1("makeCircle", (5.0,))
            .expect("Part.makeCircle(5)");
        let circumference = 2.0 * std::f64::consts::PI * 5.0;
        assert!(
            (length_of(&edge) - circumference).abs() < 1e-9,
            "the circle edge is the full circumference"
        );

        // Upstream's call: a list of curve parameters.
        let parameters = PyList::new(py, [1.0, 2.0]).expect("parameter list");
        let split = edge
            .call_method1("split", (&parameters,))
            .expect("Edge.split([1.0, 2.0])");
        let pieces = split.getattr("Edges").expect("split.Edges");
        assert_eq!(pieces.len().expect("piece count"), 3, "three pieces");
        let total: f64 = (0..3)
            .map(|index| {
                length_of(&pieces.get_item(index).expect("a split edge"))
            })
            .sum();
        assert!(
            (total - circumference).abs() < 1e-6,
            "the pieces add up to the edge's length: {total} vs {circumference}"
        );
        // Each piece is the arc between consecutive parameters, in order.
        let first = length_of(&pieces.get_item(0).expect("the first piece"));
        let second = length_of(&pieces.get_item(1).expect("the second piece"));
        let third = length_of(&pieces.get_item(2).expect("the third piece"));
        assert!((first - 5.0).abs() < 1e-9, "0 -> 1 is length 5, got {first}");
        assert!((second - 5.0).abs() < 1e-9, "1 -> 2 is length 5, got {second}");
        assert!(
            (third - 5.0 * (2.0 * std::f64::consts::PI - 2.0)).abs() < 1e-6,
            "2 -> 2pi is the rest of the circle, got {third}"
        );

        // The original edge is unchanged.
        assert!(
            (length_of(&edge) - circumference).abs() < 1e-9,
            "split does not modify the original edge"
        );

        // A single float works too (upstream's `edges[0].split(0.25)`).
        let half = edge.call_method1("split", (1.0,)).expect("Edge.split(1.0)");
        assert_eq!(
            half.getattr("Edges")
                .expect("split.Edges")
                .len()
                .expect("piece count"),
            2,
            "one parameter makes two pieces"
        );
    });
}

/// `isValid` is true for a well-formed box and false for the null shape.
#[test]
fn is_valid_reports_box_and_null() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let boxed = part
            .call_method1("makeBox", (1.0, 2.0, 2.0))
            .expect("Part.makeBox(1, 2, 2)");
        assert!(
            boxed
                .call_method0("isValid")
                .expect("Shape.isValid")
                .extract::<bool>()
                .expect("isValid as bool"),
            "a box is valid"
        );
        let null = part.getattr("Shape").expect("Part.Shape").call0().expect("Part.Shape()");
        assert!(
            !null
                .call_method0("isValid")
                .expect("null Shape.isValid")
                .extract::<bool>()
                .expect("isValid as bool"),
            "the null shape is not valid"
        );
    });
}

/// `Part.makeSplitShape(face, [(edge, face)])` returns upstream's two groups of
/// pieces.
#[test]
fn make_split_shape_returns_two_groups() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let vector = ferrocad.getattr("Vector").expect("ferrocad.Vector");

        let boxed = part
            .call_method1("makeBox", (1.0, 2.0, 2.0))
            .expect("Part.makeBox(1, 2, 2)");
        let face = boxed
            .getattr("Faces")
            .expect("Shape.Faces")
            .get_item(0)
            .expect("the first face");
        let edge = face
            .getattr("Edges")
            .expect("Face.Edges")
            .get_item(0)
            .expect("the first edge");
        let offset = vector.call1((0.0, 0.5, 0.0)).expect("a vector");
        let cutter = edge
            .call_method1("translated", (&offset,))
            .expect("Edge.translated");

        let pair = PyTuple::new(py, [&cutter, &face]).expect("a (cutter, face) pair");
        let splits = PyList::new(py, [pair]).expect("the splits list");
        let groups = part
            .call_method1("makeSplitShape", (&face, &splits))
            .expect("Part.makeSplitShape(face, [(edge, face)])");

        assert_eq!(groups.len().expect("group count"), 2, "upstream's two groups");
        let direct = groups.get_item(0).expect("the direct group");
        let left = groups.get_item(1).expect("the left group");
        assert_eq!(direct.len().expect("direct pieces"), 1);
        assert_eq!(left.len().expect("left pieces"), 1);

        for (group, name) in [(&direct, "direct"), (&left, "left")] {
            let piece = group.get_item(0).expect("a split face");
            assert!(
                number(&piece.getattr("Area").expect("Face.Area"), "Area") > 0.0,
                "the {name} piece has area"
            );
            assert!(
                piece
                    .call_method0("isValid")
                    .expect("Face.isValid")
                    .extract::<bool>()
                    .expect("isValid as bool"),
                "the {name} piece is valid"
            );
        }
    });
}

/// `Part.show` creates a `Part::Feature` whose `Shape` is the one shown.
#[test]
fn show_creates_a_feature_with_the_shape() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        ferrocad
            .call_method1("newDocument", ("LifecycleShow",))
            .expect("newDocument");

        let boxed = part
            .call_method1("makeBox", (1.0, 2.0, 2.0))
            .expect("Part.makeBox(1, 2, 2)");
        let object = part
            .call_method1("show", (&boxed, "ShownBox"))
            .expect("Part.show(box, \"ShownBox\")");

        let name: String = object
            .getattr("Name")
            .expect("DocumentObject.Name")
            .extract()
            .expect("Name as str");
        assert!(name.starts_with("ShownBox"), "the object is named after `name`, got {name}");

        let shown = object.getattr("Shape").expect("DocumentObject.Shape");
        assert!(!is_null(&shown), "the feature carries a shape");
        assert!(
            shown
                .call_method1("isSame", (&boxed,))
                .expect("Shape.isSame")
                .extract::<bool>()
                .expect("isSame as bool"),
            "the feature's Shape is the shape that was shown"
        );
        assert!(
            (number(&shown.getattr("Volume").expect("Shape.Volume"), "Volume") - 4.0).abs() < 1e-9,
            "the shown box is 1x2x2"
        );
    });
}

/// `Part.makeSweepSurface(circle, line)` is the pipe around the line.
#[test]
fn make_sweep_surface_sweeps_the_profile() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let vector = ferrocad.getattr("Vector").expect("ferrocad.Vector");

        let circle = part
            .call_method1("makeCircle", (5.0,))
            .expect("Part.makeCircle(5)");
        let start = vector.call1((0.0, 0.0, 0.0)).expect("start point");
        let end = vector.call1((0.0, 0.0, 10.0)).expect("end point");
        let path = part
            .call_method1("makeLine", (&start, &end))
            .expect("Part.makeLine");

        let surface = part
            .call_method1("makeSweepSurface", (&path, &circle, 0.001, 0))
            .expect("Part.makeSweepSurface(path, circle)");
        assert!(!is_null(&surface), "the sweep produced a shape");

        let area = number(&surface.getattr("Area").expect("Shape.Area"), "Area");
        assert!(area > 0.0, "the pipe has a positive area, got {area}");
        // A circle of radius 5 swept 10 along +Z: a 2*pi*5*10 tube.
        let expected = 2.0 * std::f64::consts::PI * 5.0 * 10.0;
        assert!(
            (area - expected).abs() < 1e-6,
            "the lateral area is {expected}, got {area}"
        );

        let bounds = surface.getattr("BoundBox").expect("Shape.BoundBox");
        let zmax = number(&bounds.getattr("ZMax").expect("BoundBox.ZMax"), "ZMax");
        assert!((zmax - 10.0).abs() < 1e-6, "the pipe reaches z=10, got {zmax}");
    });
}

/// `MatrixOfInertia` and `PrincipalProperties` for a 2x1x2 box, whose inertia
/// diagonal is (5/3, 8/3, 5/3).
#[test]
fn inertia_and_principal_properties() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let boxed = part
            .call_method1("makeBox", (2.0, 1.0, 2.0))
            .expect("Part.makeBox(2, 1, 2)");

        let matrix = boxed
            .getattr("MatrixOfInertia")
            .expect("Shape.MatrixOfInertia");
        let values: Vec<f64> = matrix.getattr("A").expect("Matrix.A").extract().expect("A as floats");
        assert_eq!(values.len(), 16, "a 4x4 matrix");
        for (index, expected) in [
            (0, 5.0 / 3.0),
            (5, 8.0 / 3.0),
            (10, 5.0 / 3.0),
            (15, 1.0),
        ] {
            assert!(
                (values[index] - expected).abs() < 1e-9,
                "MatrixOfInertia[{index}] is {}, expected {expected}",
                values[index]
            );
        }
        for (index, value) in values.iter().enumerate() {
            if !matches!(index, 0 | 5 | 10 | 15) {
                assert!(value.abs() < 1e-9, "off-diagonal {index} is {value}");
            }
        }

        let properties = boxed
            .getattr("PrincipalProperties")
            .expect("Shape.PrincipalProperties");
        let properties = properties
            .downcast::<PyDict>()
            .expect("PrincipalProperties is a dict");
        let symmetry_axis: bool = properties
            .get_item("SymmetryAxis")
            .expect("SymmetryAxis")
            .expect("SymmetryAxis is present")
            .extract()
            .expect("SymmetryAxis as bool");
        assert!(symmetry_axis, "a box has a symmetry axis");
        let symmetry_point: bool = properties
            .get_item("SymmetryPoint")
            .expect("SymmetryPoint")
            .expect("SymmetryPoint is present")
            .extract()
            .expect("SymmetryPoint as bool");
        assert!(!symmetry_point, "a box is not spherically symmetric");

        let moments: (f64, f64, f64) = properties
            .get_item("Moments")
            .expect("Moments")
            .expect("Moments is present")
            .extract()
            .expect("Moments as a tuple");
        assert!((moments.0 - 8.0 / 3.0).abs() < 1e-9, "moment 0 is {}", moments.0);
        assert!((moments.1 - 5.0 / 3.0).abs() < 1e-9, "moment 1 is {}", moments.1);
        assert!((moments.2 - 5.0 / 3.0).abs() < 1e-9, "moment 2 is {}", moments.2);

        // The axes are the coordinate axes (in the kernel's eigen order).
        let axis = |name: &str| {
            let vector = properties
                .get_item(name)
                .unwrap_or_else(|_| panic!("{name}"))
                .unwrap_or_else(|| panic!("{name} is present"));
            let component = |axis: &str| {
                number(
                    &vector.getattr(axis).unwrap_or_else(|_| panic!("{name}.{axis}")),
                    axis,
                )
            };
            (component("x"), component("y"), component("z"))
        };
        let close = |actual: (f64, f64, f64), expected: (f64, f64, f64)| {
            (actual.0 - expected.0).abs() < 1e-9
                && (actual.1 - expected.1).abs() < 1e-9
                && (actual.2 - expected.2).abs() < 1e-9
        };
        assert!(
            close(axis("FirstAxisOfInertia"), (0.0, 1.0, 0.0)),
            "the first axis is +Y, got {:?}",
            axis("FirstAxisOfInertia")
        );
        assert!(
            close(axis("SecondAxisOfInertia"), (0.0, 0.0, 1.0)),
            "the second axis is +Z, got {:?}",
            axis("SecondAxisOfInertia")
        );
        assert!(
            close(axis("ThirdAxisOfInertia"), (1.0, 0.0, 0.0)),
            "the third axis is +X, got {:?}",
            axis("ThirdAxisOfInertia")
        );
        let radii: (f64, f64, f64) = properties
            .get_item("RadiusOfGyration")
            .expect("RadiusOfGyration")
            .expect("RadiusOfGyration is present")
            .extract()
            .expect("RadiusOfGyration as a tuple");
        // Mass 4: r^2 = moment / mass.
        assert!(
            (radii.0 * radii.0 - (8.0 / 3.0) / 4.0).abs() < 1e-9,
            "radius of gyration 0 is {}",
            radii.0
        );
    });
}
